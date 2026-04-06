// HTTP Route Protocol test module
// Note: HTTPS moved to EdgionTls module

mod h2c;
mod websocket;

pub use h2c::H2cTestSuite;
pub use websocket::WebSocketTestSuite;
