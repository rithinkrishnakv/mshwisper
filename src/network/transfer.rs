//! Real chunked file transfer over the existing TCP mesh connections.
//!
//! Flow:
//!   Sender:  /mshsend <peer> <path>
//!     → sends AppMessage::FileOffer to peer
//!     → peer replies AppMessage::FileAccept or FileReject
//!     → sender streams FileChunk WirePackets
//!
//!   Receiver: popup → Y accepts → FileReceiver writes chunks to ~/Downloads/

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::core::types::WirePacket;

pub const CHUNK_SIZE: usize = 64 * 1024; // 64 KB

/// Stream a file as FileChunk WirePackets into `tx`.
/// Returns total bytes sent.
pub async fn send_file(
    path:        &str,
    transfer_id: String,
    tx:          mpsc::Sender<WirePacket>,
    progress_tx: mpsc::Sender<u64>,   // sends cumulative bytes for progress bar
) -> Result<u64> {
    let mut file  = tokio::fs::File::open(path).await?;
    let mut index = 0u64;
    let mut total = 0u64;
    let mut buf   = vec![0u8; CHUNK_SIZE];

    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 { break; }
        total   += n as u64;
        let last = n < CHUNK_SIZE;

        tx.send(WirePacket::FileChunk {
            transfer_id: transfer_id.clone(),
            chunk_index: index,
            data:        buf[..n].to_vec(),
            is_last:     last,
        }).await?;

        let _ = progress_tx.try_send(total);
        index += 1;
        if last { break; }
    }
    Ok(total)
}

/// Assembles incoming chunks and writes them to ~/Downloads/<filename>.
pub struct FileReceiver {
    pub transfer_id:    String,
    pub filename:       String,
    pub dest_path:      String,
    file:               tokio::fs::File,
    pub received_bytes: u64,
    expected_next:      u64,
}

impl FileReceiver {
    pub async fn new(transfer_id: String, filename: String) -> Result<Self> {
        let dest = downloads_path(&filename);
        if let Some(parent) = std::path::Path::new(&dest).parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let file = tokio::fs::File::create(&dest).await?;
        Ok(Self {
            transfer_id,
            filename,
            dest_path: dest,
            file,
            received_bytes: 0,
            expected_next:  0,
        })
    }

    /// Write a chunk. Returns an error if chunks arrive out of order.
    pub async fn write_chunk(&mut self, index: u64, data: &[u8]) -> Result<()> {
        if index != self.expected_next {
            anyhow::bail!("Out-of-order chunk: expected {} got {}", self.expected_next, index);
        }
        self.file.write_all(data).await?;
        self.received_bytes += data.len() as u64;
        self.expected_next  += 1;
        Ok(())
    }

    pub async fn finalize(mut self) -> Result<String> {
        self.file.flush().await?;
        Ok(self.dest_path)
    }
}

/// Cross-platform path to the user's Downloads directory.
pub fn downloads_path(filename: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOMEPATH"))
            .unwrap_or_else(|_| "C:\\Users\\Public".into());
        format!("{}\\Downloads\\{}", home, filename)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{}/Downloads/{}", home, filename)
    }
}
