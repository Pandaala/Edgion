use tonic::Request;
use futures::StreamExt;

pub mod test {
    tonic::include_proto!("test");
}

use test::test_service_client::TestServiceClient;
use test::{HelloRequest, NumberRequest};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("gRPC Test Client\n");

    // Test servers (assuming they're running on ports 30021-30024)
    let servers = vec![
        "http://127.0.0.1:30021",
        "http://127.0.0.1:30022",
        "http://127.0.0.1:30023",
        "http://127.0.0.1:30024",
    ];

    // Get server URL from command line or use first server
    let args: Vec<String> = std::env::args().collect();
    let server_url = if args.len() > 1 {
        args[1].clone()
    } else {
        println!("Usage: {} <http://server:port>", args[0]);
        println!("Using default: {}\n", servers[0]);
        servers[0].to_string()
    };

    println!("Connecting to {}...", server_url);

    // Create gRPC client
    match TestServiceClient::connect(server_url.clone()).await {
        Ok(mut client) => {
            println!("✓ Connected successfully!\n");

            // Test 1: Unary RPC - SayHello
            println!("=== Test 1: Unary RPC (SayHello) ===");
            for i in 1..=3 {
                let request = Request::new(HelloRequest {
                    name: format!("User{}", i),
                });

                println!("→ Sending: name={}", request.get_ref().name);

                match client.say_hello(request).await {
                    Ok(response) => {
                        let reply = response.into_inner();
                        println!("← Received: {} (from {})", reply.message, reply.server_addr);
                    }
                    Err(e) => {
                        println!("✗ Error: {}", e);
                    }
                }

                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }

            println!();

            // Test 2: Server Streaming RPC - StreamNumbers
            println!("=== Test 2: Server Streaming RPC (StreamNumbers) ===");
            let request = Request::new(NumberRequest { count: 5 });
            
            println!("→ Requesting stream of {} numbers", request.get_ref().count);

            match client.stream_numbers(request).await {
                Ok(response) => {
                    let mut stream = response.into_inner();
                    
                    while let Some(result) = stream.next().await {
                        match result {
                            Ok(number_response) => {
                                println!("← Received number: {}", number_response.number);
                            }
                            Err(e) => {
                                println!("✗ Stream error: {}", e);
                                break;
                            }
                        }
                    }
                    
                    println!("← Stream completed");
                }
                Err(e) => {
                    println!("✗ Error starting stream: {}", e);
                }
            }

            println!("\n✓ Test completed");
        }
        Err(e) => {
            println!("✗ Failed to connect: {}", e);
            println!("\nMake sure the gRPC server is running:");
            println!("  cargo run --example test_grpc_server");
        }
    }

    Ok(())
}

