use tokio::signal::unix::{signal, SignalKind};

pub struct StopHandle {
    pub stop_cmd_sender: tokio::sync::oneshot::Sender<()>,
}

impl StopHandle {
    /// stop the gRPC API gracefully
    pub fn stop(self) {
        if let Err(e) = self.stop_cmd_sender.send(()) {
            error!("Grpc Api thread panicked: {:?}", e);
        } else {
            info!("Grpc Api finished cleanly");
        }
    }
}

pub async fn stop_signal(grpc_stop_tx: Option<StopHandle>) {
    let mut sigint = signal(SignalKind::interrupt()).expect("shutdown_listener");
    let mut sigterm = signal(SignalKind::terminate()).expect("shutdown_listener");
    tokio::select! {
        _ = sigint.recv() => {
            info!("Received SIGINT ...");
            if let Some(grpc_handle) = grpc_stop_tx {
                grpc_handle.stop()
            }
        }
        _ = sigterm.recv() => {
            info!("Received SIGTERM ...");
            if let Some(grpc_handle) = grpc_stop_tx {
                grpc_handle.stop()
            }
        }
    }
}
