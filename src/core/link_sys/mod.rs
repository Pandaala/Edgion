//! Link external systems (ES/Kafka/ClickHouse/Redis/etc.)

mod data_sender_trait;
mod local_file_writer;

pub use data_sender_trait::DataSender;
pub use local_file_writer::LocalFileWriter;

