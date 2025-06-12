use std::path::PathBuf;
use std::sync::Arc;

use cln_rpc::model::requests::{InvoiceRequest, ListinvoicesRequest, ListpaysRequest, PayRequest};
use cln_rpc::model::responses::{
    InvoiceResponse, ListinvoicesResponse, ListpaysResponse, PayResponse,
};
use cln_rpc::ClnRpc;
use tokio::sync::{mpsc, oneshot, Mutex};

pub struct ClnConnection {
    pub pipeline: mpsc::Sender<Request>,
    worker_pool: WorkerPool,
}

impl ClnConnection {
    pub fn new(rpc_socket: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel(10_000);

        let worker_pool = WorkerPool::new(rpc_socket, 5, rx);

        Self {
            pipeline: tx,
            worker_pool,
        }
    }
}

impl Drop for ClnConnection {
    fn drop(&mut self) {
        self.worker_pool.shutdown();
    }
}

pub enum Request {
    Pay(
        PayRequest,
        oneshot::Sender<Result<PayResponse, cln_rpc::RpcError>>,
    ),
    Invoice(
        InvoiceRequest,
        oneshot::Sender<Result<InvoiceResponse, cln_rpc::RpcError>>,
    ),
    ListInvoices(
        ListinvoicesRequest,
        oneshot::Sender<Result<ListinvoicesResponse, cln_rpc::RpcError>>,
    ),
    ListPays(
        ListpaysRequest,
        oneshot::Sender<Result<ListpaysResponse, cln_rpc::RpcError>>,
    ),
}

struct WorkerPool {
    workers: Vec<tokio::task::JoinHandle<()>>,
}

macro_rules! handle_rpc_request {
    ($request:expr, $sender:expr, $cln_rpc:expr, $worker_id:expr, $socket_path:expr) => {
        let response = $cln_rpc.call_typed(&$request).await;
        if response.is_err() {
            Self::handle_rpc_error($worker_id, &$socket_path, &mut $cln_rpc).await;
        }
        if let Err(err) = $sender.send(response) {
            tracing::error!("Worker {}: Could not send response: {:?}", $worker_id, err);
        }
    };
}

impl WorkerPool {
    async fn handle_rpc_error(worker_id: usize, socket_path: &PathBuf, cln_rpc: &mut ClnRpc) {
        tracing::warn!(
            "Worker {}: RPC call failed, recreating connection",
            worker_id
        );
        match ClnRpc::new(socket_path).await {
            Ok(new_rpc) => *cln_rpc = new_rpc,
            Err(err) => {
                tracing::error!(
                    "Worker {}: Failed to recreate connection: {:?}",
                    worker_id,
                    err
                );
            }
        }
    }

    fn new(
        socket_path: PathBuf,
        worker_count: usize,
        request_receiver: mpsc::Receiver<Request>,
    ) -> Self {
        let request_receiver = Arc::new(Mutex::new(request_receiver));
        let mut workers = Vec::new();
        for worker_id in 0..worker_count {
            let socket_path = socket_path.clone();
            let receiver = Arc::clone(&request_receiver);
            let worker = tokio::spawn(async move {
                // Each worker maintains its own connection
                let mut cln_rpc = match ClnRpc::new(&socket_path).await {
                    Ok(rpc) => rpc,
                    Err(err) => {
                        tracing::error!(
                            "Worker {}: Failed to create RPC connection: {:?}",
                            worker_id,
                            err
                        );
                        return;
                    }
                };
                loop {
                    let request = {
                        let mut rx = receiver.lock().await;
                        rx.recv().await
                    };
                    match request {
                        Some(request) => match request {
                            Request::Pay(request, sender) => {
                                handle_rpc_request!(
                                    request,
                                    sender,
                                    cln_rpc,
                                    worker_id,
                                    socket_path
                                );
                            }
                            Request::Invoice(request, sender) => {
                                handle_rpc_request!(
                                    request,
                                    sender,
                                    cln_rpc,
                                    worker_id,
                                    socket_path
                                );
                            }
                            Request::ListInvoices(request, sender) => {
                                handle_rpc_request!(
                                    request,
                                    sender,
                                    cln_rpc,
                                    worker_id,
                                    socket_path
                                );
                            }
                            Request::ListPays(request, sender) => {
                                handle_rpc_request!(
                                    request,
                                    sender,
                                    cln_rpc,
                                    worker_id,
                                    socket_path
                                );
                            }
                        },
                        None => {
                            tracing::info!("Worker {}: Channel closed, shutting down", worker_id);
                            break;
                        }
                    }
                }
            });
            workers.push(worker);
        }
        Self { workers }
    }

    fn shutdown(&self) {
        for worker in self.workers.iter() {
            worker.abort();
        }
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        for worker in self.workers.iter() {
            worker.abort();
        }
    }
}
