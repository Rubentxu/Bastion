//! Streaming output types for command execution.

use serde::{Deserialize, Serialize};

/// Type of output chunk from a running command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkType {
    Stdout,
    Stderr,
    ExitCode,
    Progress,
    Error,
}

/// A chunk of output from a running command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandChunk {
    pub chunk_type: ChunkType,
    pub data: Vec<u8>,
    pub is_final: bool,
}

impl CommandChunk {
    pub fn stdout(data: impl Into<Vec<u8>>) -> Self {
        Self {
            chunk_type: ChunkType::Stdout,
            data: data.into(),
            is_final: false,
        }
    }

    pub fn stderr(data: impl Into<Vec<u8>>) -> Self {
        Self {
            chunk_type: ChunkType::Stderr,
            data: data.into(),
            is_final: false,
        }
    }

    pub fn exit_code(code: i32) -> Self {
        Self {
            chunk_type: ChunkType::ExitCode,
            data: code.to_le_bytes().to_vec(),
            is_final: true,
        }
    }

    pub fn progress(percent: u8) -> Self {
        Self {
            chunk_type: ChunkType::Progress,
            data: vec![percent],
            is_final: false,
        }
    }

    pub fn error(message: impl Into<Vec<u8>>) -> Self {
        Self {
            chunk_type: ChunkType::Error,
            data: message.into(),
            is_final: true,
        }
    }
}
