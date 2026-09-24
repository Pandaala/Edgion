#[cfg(test)]
#[derive(Default)]
struct ScriptedRoute {
    queue: std::collections::VecDeque<ScriptedEvent>,
    repeat: Option<ScriptedEvent>,
    live_jwks: Option<LiveJwks>,
    hits: usize,
}
