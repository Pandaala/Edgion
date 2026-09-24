#[cfg(test)]
#[derive(Clone)]
enum ScriptedEvent {
    Http { status: u16, body: Vec<u8> },
    Timeout,
    Transport,
}
