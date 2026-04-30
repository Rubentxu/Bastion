//! Worker service implementation - runs inside each sandbox as a gRPC server.

use crate::sandbox::v1::worker_agent_server::WorkerAgent;
use crate::sandbox::v1::*;
use std::pin::Pin;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

pub struct WorkerService;

#[tonic::async_trait]
impl WorkerAgent for WorkerService {
    type RunCommandStream =
        Pin<Box<dyn tokio_stream::Stream<Item = Result<RunCommandResponse, Status>> + Send>>;

    async fn run_command(
        &self,
        request: Request<RunCommandRequest>,
    ) -> Result<Response<Self::RunCommandStream>, Status> {
        let req = request.into_inner();
        let (tx, rx) = mpsc::channel(32);

        let command = req.command;
        let args = req.args;

        tokio::spawn(async move {
            let mut cmd = if args.is_empty() {
                let mut c = Command::new("sh");
                c.arg("-c").arg(&command);
                c
            } else {
                let mut c = Command::new(&command);
                for arg in &args {
                    c.arg(arg);
                }
                c
            };

            let mut child = cmd
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("Failed to spawn command");

            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();

            // Read stdout and stderr in parallel
            let tx1 = tx.clone();
            let tx2 = tx.clone();

            let h1 = tokio::spawn(async move {
                let mut reader = stdout;
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let _ = tx1
                                .send(Ok(RunCommandResponse {
                                    r#type: 0, // STDOUT
                                    data: buf[..n].to_vec(),
                                }))
                                .await;
                        }
                        Err(_) => break,
                    }
                }
            });

            let h2 = tokio::spawn(async move {
                let mut reader = stderr;
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let _ = tx2
                                .send(Ok(RunCommandResponse {
                                    r#type: 1, // STDERR
                                    data: buf[..n].to_vec(),
                                }))
                                .await;
                        }
                        Err(_) => break,
                    }
                }
            });

            // Wait for process
            let status = child.wait().await.expect("Failed to wait for child");
            let _ = h1.await;
            let _ = h2.await;

            // Send exit code
            let code = status.code().unwrap_or(-1);
            let _ = tx
                .send(Ok(RunCommandResponse {
                    r#type: 2, // EXIT_CODE
                    data: code.to_le_bytes().to_vec(),
                }))
                .await;
        });

        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn read_file(
        &self,
        request: Request<ReadFileRequest>,
    ) -> Result<Response<ReadFileResponse>, Status> {
        let req = request.into_inner();
        let content =
            tokio::fs::read(&req.path)
                .await
                .map_err(|e| Status::internal(format!("Failed to read file: {}", e)))?;
        Ok(Response::new(ReadFileResponse { content }))
    }

    async fn write_file(
        &self,
        request: Request<WriteFileRequest>,
    ) -> Result<Response<WriteFileResponse>, Status> {
        let req = request.into_inner();
        if let Some(parent) = std::path::Path::new(&req.path).parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| Status::internal(format!("Failed to create dir: {}", e)))?;
        }
        tokio::fs::write(&req.path, &req.content)
            .await
            .map_err(|e| Status::internal(format!("Failed to write file: {}", e)))?;
        Ok(Response::new(WriteFileResponse {
            bytes_written: req.content.len() as i64,
        }))
    }

    async fn list_files(
        &self,
        request: Request<ListFilesRequest>,
    ) -> Result<Response<ListFilesResponse>, Status> {
        let req = request.into_inner();
        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&req.directory)
            .await
            .map_err(|e| Status::internal(format!("Failed to read dir: {}", e)))?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| Status::internal(format!("Failed to read entry: {}", e)))?
        {
            let path = entry.path();
            let file_type = entry.file_type().await
                .map_err(|e| Status::internal(format!("Failed to get file type: {}", e)))?;
            let size_bytes = tokio::fs::metadata(&path).await
                .map(|m| m.len() as i64)
                .unwrap_or(0);
            let permissions = tokio::fs::metadata(&path).await
                .map(|m| format!("{:?}", m.permissions()))
                .unwrap_or_else(|_| "unknown".to_string());

            entries.push(FileEntry {
                path: path.to_string_lossy().to_string(),
                is_directory: file_type.is_dir(),
                size_bytes,
                permissions,
            });
        }
        Ok(Response::new(ListFilesResponse { entries }))
    }

    async fn health(&self, _request: Request<HealthRequest>) -> Result<Response<HealthResponse>, Status> {
        Ok(Response::new(HealthResponse {
            status: "healthy".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }
}
