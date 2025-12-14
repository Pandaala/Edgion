use tonic::{transport::Server, Request, Response, Status};
use tokio::task;
use futures::future;
use std::net::SocketAddr;

pub mod test {
    tonic::include_proto!("test");
}

use test::test_service_server::{TestService, TestServiceServer};
use test::{HelloRequest, HelloResponse, NumberRequest, NumberResponse};

#[derive(Debug)]
pub struct TestServiceImpl {
    server_addr: String,
}

impl TestServiceImpl {
    fn new(server_addr: String) -> Self {
        Self { server_addr }
    }
}

#[tonic::async_trait]
impl TestService for TestServiceImpl {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloResponse>, Status> {
        let name = request.into_inner().name;
        println!("[{}] Received SayHello request: name={}", self.server_addr, name);

        let response = HelloResponse {
            message: format!("Hello, {}!", name),
            server_addr: self.server_addr.clone(),
        };

        println!("[{}] Sending response: {}", self.server_addr, response.message);
        Ok(Response::new(response))
    }

    type StreamNumbersStream = tokio_stream::wrappers::ReceiverStream<Result<NumberResponse, Status>>;

    async fn stream_numbers(
        &self,
        request: Request<NumberRequest>,
    ) -> Result<Response<Self::StreamNumbersStream>, Status> {
        let count = request.into_inner().count;
        println!("[{}] Received StreamNumbers request: count={}", self.server_addr, count);

        let (tx, rx) = tokio::sync::mpsc::channel(10);

        let server_addr = self.server_addr.clone();
        tokio::spawn(async move {
            for i in 1..=count {
                println!("[{}] Streaming number: {}", server_addr, i);
                
                let response = NumberResponse { number: i };
                
                if tx.send(Ok(response)).await.is_err() {
                    println!("[{}] Client disconnected", server_addr);
                    break;
                }
                
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
            println!("[{}] Stream completed", server_addr);
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

#[tokio::main]
async fn main() {
    println!("Starting gRPC test servers...\n");

    let addrs = [
        "127.0.0.1:30021".parse::<SocketAddr>().unwrap(),
        "127.0.0.1:30022".parse::<SocketAddr>().unwrap(),
        "127.0.0.1:30023".parse::<SocketAddr>().unwrap(),
        "127.0.0.1:30024".parse::<SocketAddr>().unwrap(),
    ];

    println!("gRPC servers will listen on:");
    for addr in &addrs {
        println!("  - http://{}", addr);
    }
    println!();

    let servers = [
        task::spawn(run_server(addrs[0])),
        task::spawn(run_server(addrs[1])),
        task::spawn(run_server(addrs[2])),
        task::spawn(run_server(addrs[3])),
    ];

    future::join_all(servers).await;
}

async fn run_server(addr: SocketAddr) {
    let service = TestServiceImpl::new(addr.to_string());
    
    println!("gRPC server listening on http://{}", addr);
    
    if let Err(e) = Server::builder()
        .add_service(TestServiceServer::new(service))
        .serve(addr)
        .await
    {
        eprintln!("[{}] Server error: {}", addr, e);
    }
}

