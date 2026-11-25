# Lessons

Things learned while building Ferrogram, newest first.

- **A self-updater must hand off the single-instance lock.** If the new process
  starts while the old one still owns the socket, it just shows the old window and
  exits. `shutdown()` on the listening socket wakes the blocked `accept()`, so the
  thread can drop the listener before the new binary is spawned.
- **Spawn work threads inside the task's future, not when building the task.**
  Tests drive `update()` and drop the returned `Task`; a thread started eagerly
  would hit the network anyway.
- **Never follow redirects blindly for downloads.** Follow them by hand and check
  every hop's host (GitHub sends assets to its storage hosts with signed query
  strings, which must not end up in logs either).

- **Check what the official apps register before choosing a call protocol.**
  libtgvoip (2.4.4) is still the fallback string in Telegram Desktop but no
  app registers it any more; the server picks one version from both lists,
  so advertising only legacy versions fails silently. tgcalls V2
  (8.0.0/9.0.0) is the common ground.
- **Write protocol vectors with an independent tool.** STUN, SRTP and the
  call KDF tests use values from Python hmac/hashlib and `openssl enc`, not
  from memory: a remembered RFC 5769 MAC turned out to be wrong.
- **`tokio::spawn` needs a runtime: start per-call engines inside a `Task`
  future**, never in `update()`, and bring handles back through an
  `Arc<Mutex<Option<_>>>` slot in the message.
- **On tiny-skia, an image is only cheap when it is drawn at its own size.**
  Every `draw_image` whose destination differs from the source runs a
  bilinear pipeline on the CPU, per frame, and the renderer drops decoded
  images that were not drawn in the last frame, so a grid of 20 oversized
  previews cost ~2.5x the frame time of 1:1 tiles (measured, debug build).
  Decode and crop previews to the physical cell size off the UI thread, build
  one `Handle::from_rgba` each, and keep a fixed row height so the view can
  skip rows outside the viewport.
- **A muted chat is not a boolean.** `mute_until` can be unset (use the
  default of that kind of chat), in the past (unmuted) or in the future, and
  the account defaults are a separate call. Derive `muted` in one place and
  send an explicit `0` to unmute, or a "muted by default" setting wins again.
- **Benchmarks that reuse one frame measure the cache, not the scroll.**
  Feed wheel events through a persisted `UserInterface` cache and deliver the
  `on_scroll` messages, or new rows never get drawn for the first time.
- **`messages.deleteHistory` answers "more to do".** Repeat while
  `offset > 0`, with a hard cap.

- **A modal backdrop must be `opaque`.** iced's `stack` only hides the cursor
  from the layers below when the top layer reports a mouse interaction, and a
  plain `mouse_area` reports none outside its content: the chat rows under
  the Settings dim kept their hover highlight and pointer shape. Use
  `widgets::modal`; `tests/ui_layers.rs` probes the real view headlessly.
- **svg handles hash their whole document on every `from_memory`.** Build icon
  handles once (`icons::handle`), not per row in `view()`.
- **X11 clipboard data dies with the last `arboard::Clipboard`.** Keep one
  long-lived handle, or "Copy image" pastes nothing.
- **A file copied in a file manager is also text.** The composer has already
  pasted the path when the async clipboard read says "file": undo that paste.

- **Never rely on a system emoji font.** Bundle Noto Color Emoji (same family name as
  the system one, so either works) and load it only when no system copy exists (it costs
  ~15-20 MB RSS). Classify emoji per grapheme cluster (keycaps, flags, ZWJ, skin tones, VS16),
  not per char range, and truncate on cluster boundaries. Test with the system font hidden.
- **dbusmenu ids are renumbered whenever the menu changes** (ksni offsets
  them), and zbus proxies cache properties: tests must read `GetLayout` first
  and use uncached proxies.
- **`exit_on_close_request: false` + `iced::daemon`** is what makes "close to
  tray" possible; answer `CloseRequested` yourself or the app exits.
- **Never hide without a tray host.** Registration can fail or the panel can
  restart; a hidden window with no tray is an unreachable process.

- **iced's `image::border_radius` is not drawn by the tiny-skia renderer.**
  Round avatars by masking the pixels (alpha) when decoding, so every
  renderer agrees. Decode on a blocking thread.
- **"Small blobs are fine" is the wrong default for a cache validator.** A
  corrupt cache file passed the old photo check and would be drawn blank
  forever; avatars now require a parseable JPEG/PNG header.
- **Keep the dialog iterator.** `iter_dialogs()` restarts from the top each
  time; holding it between calls makes "load more" a plain continuation. The
  prune-on-refresh step must then run only when the first page was complete.
- **A search hit needs a detached window.** Appending live messages to a
  window around an old message creates an invisible gap. Mark it detached and
  offer "jump to latest".
- **Typing indicators and draft sync are best effort.** They fail in
  channels you cannot post in; log, never toast.
- **Executable detection looks at the last extension only after stripping
  trailing dots and spaces** (`evil.exe.`), and file names from the network
  are sanitised before they touch the disk (separators, controls, bidi
  overrides).

- **Embed app credentials with build.rs, not only `option_env!`.** A build
  script can validate the values (and fail the build on a typo),
  `cargo:rerun-if-env-changed` rebuilds when they change, and the values
  never appear in build output. Plain `option_env!` would happily ship
  `"12a"` as an api_id.
- **The api_hash is an app identifier, not a user secret.** Every Telegram
  client ships one, and it can be extracted from the binary. Users should
  never have to see it: the override belongs behind an "Advanced" link.
- **Make credential resolution a pure function over injectable inputs**
  (keyring, env lookup, embedded values) so CI covers every path without
  real secrets.

- **Take-once channels break re-login.** An event receiver or update pump
  taken on first use silently stops delivering events after logout → login.
  Make subscriptions restartable (`EventHub`), and rebuild the protocol
  client on logout/revoke.
- **Offline ≠ logged out.** Treating a network error from `restore()` as
  "show the phone screen" costs a fresh SMS, risks FLOOD_WAIT and can
  overwrite a good session. Retry transient errors with backoff and keep the
  cache visible.
- **A cache is not a history.** Serving "older" pages from SQLite without
  knowing that the range is contiguous hides gaps. Track network-confirmed
  ranges per chat.
- **`tokio::time::sleep()` needs a runtime when called, not when awaited.**
  Wrap it in `async { … }` inside `Task::perform`, or unit tests that drive
  `update()` without a runtime panic.
- **Notification bodies are markup** on KDE/freedesktop. Escape `& < >` in
  both title and body, and coalesce bursts (catch-up after a reconnect).
- **Check image headers before decoding** and cap bytes and pixels, so a
  hostile "photo" cannot exhaust memory.

- **grammers-session's SQLite storage clashes with rusqlite.** Its default
  `sqlite-storage` feature pulls `libsql` (another SQLite) and writes the auth
  key unencrypted. Disable default features and implement `Session` yourself;
  ~150 lines (`net/session.rs`) with serde support from the `serde` feature.
- **grammers `Dialog::peer_ref()` can panic** (`auth().unwrap()`), and
  `Peer::auth()` is crate-private. Use `peer().to_ref().await`.
- **No need for tdlib-rs.** grammers 0.12 covers login with SRP, dialogs,
  history, media upload/download, edits, deletes, reactions, search and an
  ordered update stream with gap recovery.
- **TL fixtures are cheap to make "recorded"**: build the struct, call
  `to_bytes()`, then `from_bytes()` — the deserialiser runs as if the bytes
  came off the wire.
- **iced 0.14 layout:** a `container` whose child row contains a `Fill`
  space does not always expand; set `.width(Length::Fill)` explicitly or the
  background only covers part of the bar.
- **Colour emoji:** cosmic-text's fallback may pick a monochrome font for
  emoji. Split text into emoji/non-emoji runs and give emoji spans the
  `Noto Color Emoji` font via `rich_text`.
- **No ellipsis in iced text.** Truncate by chars and set
  `Wrapping::None` + `container.clip(true)` for single-line previews.
- **Redaction writer vs timestamps:** a naive "mask digit runs with dashes"
  rule also masked ISO dates in log lines. Only accept separators after a
  leading `+`.
- **Deterministic screenshots:** pin "now" (`mock::MOCK_NOW`) and the time
  zone offset in mock mode, and drive the UI with a timed message script.
- Bot pushes made with `GITHUB_TOKEN` never trigger `push` workflows, so the release workflow chains off the version workflow with `workflow_run` (conclusion `success`, branch `main`, checkout `ref: main` for the bump commit).
- `keyring` 4 opens a *blocking* zbus connection: if any dependency turns on zbus `tokio` (ksni default features, `rfd`/`ashpd` `tokio`, a `zbus` dev-dep) it panics ("Cannot start a runtime from within a runtime") when called on a runtime worker. Keep zbus on `async-io` (`ksni` with `default-features = false, features = ["async-io"]`, `rfd` with `async-std`); `tests/real_keyring.rs` guards it against a real Secret Service.
