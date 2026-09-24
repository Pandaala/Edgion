#[cfg(test)]
pub(crate) static I5_DISPATCH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
