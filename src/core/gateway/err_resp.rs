//! Error response utilities
//!
//! Provides helper functions to send error responses in pingora handlers.

use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use bytes::Bytes;
use crate::types::EdgionHttpContext;
use crate::core::gateway::server_header::ServerHeaderOpts;

/// Send 400 Bad Request error response (nginx-style)
pub async fn end_response_400(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    ctx.request_info.status = 400;
    
    let mut resp = ResponseHeader::build(400, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/html").unwrap();
    
    let body = r#"<html>
<head><title>400 Bad Request</title></head>
<body>
<center><h1>400 Bad Request</h1></center>
<hr><center>edgion</center>
</body>
</html>"#;
    
    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
    session.shutdown().await;
    
    Ok(())
}

/// Send 421 Misdirected Request error response (RFC 7540)
/// Used when SNI and Host header mismatch for HTTPS requests
pub async fn end_response_421(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    ctx.request_info.status = 421;
    
    let mut resp = ResponseHeader::build(421, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/html").unwrap();
    
    let body = r#"<html>
<head><title>421 Misdirected Request</title></head>
<body>
<center><h1>421 Misdirected Request</h1></center>
<hr><center>edgion</center>
</body>
</html>"#;
    
    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
    session.shutdown().await;
    
    Ok(())
}

/// Send 404 Not Found error response (nginx-style)
pub async fn end_response_404(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    ctx.request_info.status = 404;
    
    let mut resp = ResponseHeader::build(404, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/html").unwrap();
    
    let body = r#"<html>
<head><title>404 Not Found</title></head>
<body>
<center><h1>404 Not Found</h1></center>
<hr><center>edgion</center>
</body>
</html>"#;
    
    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
    session.shutdown().await;
    
    Ok(())
}

/// Send 500 Internal Server Error error response (nginx-style)
pub async fn end_response_500(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    ctx.request_info.status = 500;
    
    let mut resp = ResponseHeader::build(500, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/html").unwrap();
    
    let body = r#"<html>
<head><title>500 Internal Server Error</title></head>
<body>
<center><h1>500 Internal Server Error</h1></center>
<hr><center>edgion</center>
</body>
</html>"#;
    
    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
    session.shutdown().await;
    
    Ok(())
}

/// Send 503 Service Temporarily Unavailable error response (nginx-style)
pub async fn end_response_503(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    ctx.request_info.status = 503;
    
    let mut resp = ResponseHeader::build(503, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/html").unwrap();
    
    let body = r#"<html>
<head><title>503 Service Temporarily Unavailable</title></head>
<body>
<center><h1>503 Service Temporarily Unavailable</h1></center>
<hr><center>edgion</center>
</body>
</html>"#;
    
    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
    session.shutdown().await;
    
    Ok(())
}
