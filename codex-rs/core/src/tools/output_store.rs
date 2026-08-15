use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use uuid::Uuid;

use codex_tools::RecoverableToolOutput;
use codex_utils_output_truncation::approx_bytes_for_tokens;
use codex_utils_output_truncation::approx_token_count;

const MAX_OUTPUTS: usize = 64;
const MAX_OUTPUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_TOTAL_OUTPUT_BYTES: usize = 128 * 1024 * 1024;
const STREAM_CAPTURE_QUEUE_CAPACITY: usize = 4096;
pub(crate) const MAX_PATTERN_BYTES: usize = 4 * 1024;
pub(crate) const MAX_CONTEXT_LINES: usize = 20;
const MAX_SEARCH_MATCHES: usize = 100;

/// Session-owned storage for recoverable textual tool output.
///
/// Handles are random, opaque capabilities. Their backing files are confined
/// to the session-owned temporary directory and are deleted when this store is
/// dropped with the session.
pub(crate) struct ToolOutputStore {
    temp_dir: TempDir,
    state: Mutex<StoreState>,
    limits: ToolOutputStoreLimits,
}

#[derive(Default)]
struct StoreState {
    outputs: HashMap<String, StoredOutput>,
    total_bytes: usize,
}

struct StoredOutput {
    path: PathBuf,
    byte_len: usize,
}

/// A file-backed tee for a live process stream.
///
/// The process owns the stream for as long as it runs. Once stream storage
/// reaches a quota, capturing stops without delaying process output delivery.
#[derive(Clone)]
pub(crate) struct ToolOutputStream {
    store: Arc<ToolOutputStore>,
    handle: String,
    disabled: Arc<AtomicBool>,
    pending_chunks: Arc<AtomicUsize>,
    flushed: Arc<Notify>,
    sender: mpsc::Sender<Vec<u8>>,
}

impl std::fmt::Debug for ToolOutputStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ToolOutputStream")
    }
}

#[derive(Clone, Copy)]
struct ToolOutputStoreLimits {
    max_outputs: usize,
    max_output_bytes: usize,
    max_total_output_bytes: usize,
}

impl Default for ToolOutputStoreLimits {
    fn default() -> Self {
        Self {
            max_outputs: MAX_OUTPUTS,
            max_output_bytes: MAX_OUTPUT_BYTES,
            max_total_output_bytes: MAX_TOTAL_OUTPUT_BYTES,
        }
    }
}

pub(crate) type StoredToolOutput = RecoverableToolOutput;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ToolOutputRead {
    pub(crate) content: String,
    pub(crate) next_offset: Option<usize>,
    pub(crate) total_tokens: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ToolOutputSearch {
    pub(crate) matches: Vec<ToolOutputSearchMatch>,
    pub(crate) total_matches: usize,
    pub(crate) truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ToolOutputSearchMatch {
    pub(crate) line_number: usize,
    pub(crate) context_start_line: usize,
    pub(crate) context_end_line: usize,
    pub(crate) content: String,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum ToolOutputStoreError {
    #[error("output handle is invalid or has expired")]
    Unavailable,
    #[error("tool output exceeds the per-output storage limit")]
    OutputTooLarge,
    #[error("tool output storage limit reached")]
    StorageFull,
    #[error("pattern must not be empty")]
    EmptyPattern,
    #[error("pattern exceeds the maximum size")]
    PatternTooLarge,
    #[error("failed to store tool output")]
    StorageFailed,
}

impl ToolOutputStore {
    pub(crate) fn new() -> Result<Arc<Self>, ToolOutputStoreError> {
        Self::new_with_limits(ToolOutputStoreLimits::default())
    }

    fn new_with_limits(limits: ToolOutputStoreLimits) -> Result<Arc<Self>, ToolOutputStoreError> {
        let temp_dir = tempfile::Builder::new()
            .prefix("codex-tool-output-")
            .tempdir()
            .map_err(|_| ToolOutputStoreError::StorageFailed)?;
        Ok(Arc::new(Self {
            temp_dir,
            state: Mutex::new(StoreState::default()),
            limits,
        }))
    }

    pub(crate) async fn store_text(
        &self,
        content: String,
    ) -> Result<StoredToolOutput, ToolOutputStoreError> {
        let byte_len = content.len();
        let token_count = approx_token_count(&content);
        if byte_len > self.limits.max_output_bytes {
            return Err(ToolOutputStoreError::OutputTooLarge);
        }

        let handle = format!("out_{}", Uuid::new_v4().simple());
        let path = self.temp_dir.path().join(format!("{handle}.txt"));
        {
            let mut state = self.state.lock().await;
            if state.outputs.len() >= self.limits.max_outputs
                || state.total_bytes.saturating_add(byte_len) > self.limits.max_total_output_bytes
            {
                return Err(ToolOutputStoreError::StorageFull);
            }
            state.total_bytes += byte_len;
            state.outputs.insert(
                handle.clone(),
                StoredOutput {
                    path: path.clone(),
                    byte_len,
                },
            );
        }

        if tokio::fs::write(&path, content).await.is_err() {
            let mut state = self.state.lock().await;
            if let Some(output) = state.outputs.remove(&handle) {
                state.total_bytes = state.total_bytes.saturating_sub(output.byte_len);
            }
            return Err(ToolOutputStoreError::StorageFailed);
        }
        Ok(StoredToolOutput {
            handle,
            byte_len,
            token_count,
        })
    }

    /// Starts capturing a process stream before its terminal preview buffer.
    pub(crate) async fn start_stream(
        self: &Arc<Self>,
    ) -> Result<ToolOutputStream, ToolOutputStoreError> {
        let handle = format!("out_{}", Uuid::new_v4().simple());
        let path = self.temp_dir.path().join(format!("{handle}.txt"));
        tokio::fs::File::create(&path)
            .await
            .map_err(|_| ToolOutputStoreError::StorageFailed)?;

        let mut state = self.state.lock().await;
        if state.outputs.len() >= self.limits.max_outputs {
            return Err(ToolOutputStoreError::StorageFull);
        }
        state
            .outputs
            .insert(handle.clone(), StoredOutput { path, byte_len: 0 });
        let (sender, mut receiver) = mpsc::channel(STREAM_CAPTURE_QUEUE_CAPACITY);
        let stream = ToolOutputStream {
            store: Arc::clone(self),
            handle,
            disabled: Arc::new(AtomicBool::new(false)),
            pending_chunks: Arc::new(AtomicUsize::new(0)),
            flushed: Arc::new(Notify::new()),
            sender,
        };
        let writer = stream.clone();
        tokio::spawn(async move {
            while let Some(bytes) = receiver.recv().await {
                if !writer.disabled.load(Ordering::Acquire)
                    && writer
                        .store
                        .append_stream(&writer.handle, &bytes)
                        .await
                        .is_err()
                {
                    writer.disabled.store(true, Ordering::Release);
                    writer.store.remove_output(&writer.handle).await;
                }
                writer.pending_chunks.fetch_sub(1, Ordering::AcqRel);
                writer.flushed.notify_waiters();
            }
        });
        Ok(stream)
    }

    async fn append_stream(&self, handle: &str, bytes: &[u8]) -> Result<(), ToolOutputStoreError> {
        let path = {
            let mut state = self.state.lock().await;
            let new_total = state.total_bytes.saturating_add(bytes.len());
            if new_total > self.limits.max_total_output_bytes {
                return Err(ToolOutputStoreError::StorageFull);
            }
            let output = state
                .outputs
                .get_mut(handle)
                .ok_or(ToolOutputStoreError::Unavailable)?;
            let new_output_len = output.byte_len.saturating_add(bytes.len());
            if new_output_len > self.limits.max_output_bytes {
                return Err(ToolOutputStoreError::OutputTooLarge);
            }
            output.byte_len = new_output_len;
            let path = output.path.clone();
            state.total_bytes = new_total;
            path
        };

        let mut file = match tokio::fs::OpenOptions::new().append(true).open(&path).await {
            Ok(file) => file,
            Err(_) => {
                self.remove_output(handle).await;
                return Err(ToolOutputStoreError::StorageFailed);
            }
        };
        if file.write_all(bytes).await.is_err() {
            self.remove_output(handle).await;
            return Err(ToolOutputStoreError::StorageFailed);
        }
        Ok(())
    }

    async fn stream_output(&self, handle: &str) -> Option<StoredToolOutput> {
        let state = self.state.lock().await;
        let output = state.outputs.get(handle)?;
        Some(StoredToolOutput {
            handle: handle.to_string(),
            byte_len: output.byte_len,
            token_count: output.byte_len.saturating_add(3) / 4,
        })
    }

    pub(crate) async fn read(
        &self,
        handle: &str,
        offset: usize,
        max_tokens: usize,
    ) -> Result<ToolOutputRead, ToolOutputStoreError> {
        let content = self.read_content(handle).await?;
        let total_tokens = approx_token_count(&content);
        let total_bytes = content.len();
        let start = char_boundary_at_or_before(&content, approx_bytes_for_tokens(offset));
        let end = char_boundary_at_or_before(
            &content,
            start.saturating_add(approx_bytes_for_tokens(max_tokens)),
        );
        let content = content[start..end].to_string();
        let next_offset =
            (end < total_bytes).then(|| approx_token_count(&content).saturating_add(offset));
        Ok(ToolOutputRead {
            content,
            next_offset,
            total_tokens,
        })
    }

    pub(crate) async fn search(
        &self,
        handle: &str,
        pattern: &str,
        context_lines: usize,
        max_tokens: usize,
    ) -> Result<ToolOutputSearch, ToolOutputStoreError> {
        if pattern.is_empty() {
            return Err(ToolOutputStoreError::EmptyPattern);
        }
        if pattern.len() > MAX_PATTERN_BYTES {
            return Err(ToolOutputStoreError::PatternTooLarge);
        }

        let content = self.read_content(handle).await?;
        let lines = content.lines().collect::<Vec<_>>();
        let context_lines = context_lines.min(MAX_CONTEXT_LINES);
        let mut matches = Vec::new();
        let mut total_matches = 0;
        let mut consumed_tokens: usize = 0;
        let mut truncated = false;

        for (index, line) in lines.iter().enumerate() {
            if !line.contains(pattern) {
                continue;
            }
            total_matches += 1;
            if matches.len() >= MAX_SEARCH_MATCHES {
                truncated = true;
                continue;
            }

            let context_start = index.saturating_sub(context_lines);
            let context_end = index
                .saturating_add(context_lines)
                .min(lines.len().saturating_sub(1));
            let context = lines[context_start..=context_end].join("\n");
            let context_tokens = approx_token_count(&context);
            if consumed_tokens.saturating_add(context_tokens) > max_tokens {
                truncated = true;
                continue;
            }
            consumed_tokens += context_tokens;
            matches.push(ToolOutputSearchMatch {
                line_number: index + 1,
                context_start_line: context_start + 1,
                context_end_line: context_end + 1,
                content: context,
            });
        }

        Ok(ToolOutputSearch {
            matches,
            total_matches,
            truncated,
        })
    }

    async fn read_content(&self, handle: &str) -> Result<String, ToolOutputStoreError> {
        let path = {
            let state = self.state.lock().await;
            state
                .outputs
                .get(handle)
                .map(|output| output.path.clone())
                .ok_or(ToolOutputStoreError::Unavailable)?
        };
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|_| ToolOutputStoreError::Unavailable)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    async fn remove_output(&self, handle: &str) {
        let path = {
            let mut state = self.state.lock().await;
            let Some(output) = state.outputs.remove(handle) else {
                return;
            };
            state.total_bytes = state.total_bytes.saturating_sub(output.byte_len);
            output.path
        };
        let _ = tokio::fs::remove_file(path).await;
    }
}

impl ToolOutputStream {
    /// Appends raw output before the unified-exec head/tail preview buffer.
    pub(crate) async fn append(&self, bytes: Vec<u8>) {
        if self.disabled.load(Ordering::Acquire) {
            return;
        }
        self.pending_chunks.fetch_add(1, Ordering::AcqRel);
        if self.sender.try_send(bytes).is_err() {
            self.pending_chunks.fetch_sub(1, Ordering::AcqRel);
            self.disabled.store(true, Ordering::Release);
            self.store.remove_output(&self.handle).await;
            self.flushed.notify_waiters();
        }
    }

    /// Returns the opaque handle only while the complete stream remains stored.
    pub(crate) async fn recoverable_output(&self) -> Option<StoredToolOutput> {
        loop {
            let flushed = self.flushed.notified();
            if self.pending_chunks.load(Ordering::Acquire) == 0 {
                break;
            }
            flushed.await;
        }
        (!self.disabled.load(Ordering::Acquire))
            .then(|| self.store.stream_output(&self.handle))?
            .await
    }

    /// Removes an unused stream capture after its output fits in history.
    pub(crate) async fn discard(&self) {
        self.disabled.store(true, Ordering::Release);
        self.store.remove_output(&self.handle).await;
    }
}

fn char_boundary_at_or_before(content: &str, offset: usize) -> usize {
    let mut index = offset.min(content.len());
    while index > 0 && !content.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
#[path = "output_store_tests.rs"]
mod tests;
