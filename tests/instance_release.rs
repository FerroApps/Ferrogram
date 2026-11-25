//! The single-instance socket can be given up for a restart after an
//! update, and the next process becomes the primary (own test binary: the
//! held socket is process-wide state).
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[cfg(target_os = "linux")]
#[test]
fn release_frees_the_name_for_the_next_process() {
    use ferrogram::instance::{Ask, Role, acquire, release, socket_name};
    use std::sync::Arc;

    let name = socket_name(
        &format!("dev.ferro.test-rel-{}", std::process::id()),
        0,
        false,
    );
    let sink: Arc<dyn Fn(ferrogram::remote::Remote) + Send + Sync> = Arc::new(|_| {});
    assert_eq!(acquire(&name, Ask::Show, sink.clone()), Role::Primary);
    // While held, a second launch is told to go away.
    assert_eq!(acquire(&name, Ask::Nothing, sink.clone()), Role::Secondary);
    let released = release().expect("held");
    assert_eq!(released.name, name);
    assert!(release().is_none(), "nothing held any more");
    // The name is free: the restarted process becomes the primary.
    assert_eq!(acquire(&name, Ask::Show, released.sink), Role::Primary);
}
