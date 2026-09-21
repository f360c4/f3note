//! Single-instance fallback over a unix socket.
//!
//! `GApplication` already does single-instance, and when a session bus is
//! present it does it well: a second `f3note file.txt` reaches the running
//! process in well under a tenth of a second. The problem is what happens
//! without one. With no session bus — Hyprland started from a tty, a system
//! without systemd, a broken `DBUS_SESSION_BUS_ADDRESS` — `GApplication` does
//! not fail. It prints one warning nobody reads and *every* process becomes
//! primary. Two windows, and, far worse for this editor, two processes writing
//! the same crash-recovery state.
//!
//! This module closes that hole. The socket lives in Linux's abstract
//! namespace, which is exactly right here: an abstract address has no
//! filesystem entry, is released automatically when the process dies, and
//! cannot be left behind stale by a crash or a `kill -9`. That removes the
//! usual stale-socket dance — checking whether the owner is alive, unlinking,
//! racing another process doing the same — because the condition simply cannot
//! arise. Binding either succeeds, meaning nothing else is running, or it does
//! not, meaning something is.

use std::io::{BufRead, BufReader, Write};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::path::PathBuf;

/// Abstract socket name identifying one editor instance.
///
/// The name is derived from the state directory rather than from the user, and
/// that is deliberate. Two f3note processes are only "the same instance" if
/// they would write the same crash-recovery state — which is exactly what the
/// single-instance rule exists to prevent two processes from doing. Someone
/// who runs f3note with a different `XDG_STATE_HOME` genuinely wants a
/// separate editor, and scoping by state directory gives them one. It also
/// keeps different users apart, since their state directories differ anyway.
fn address() -> std::io::Result<SocketAddr> {
    let state = crate::paths::state_dir();
    let digest = crate::session::store::hash_of(state.to_string_lossy().as_bytes());
    SocketAddr::from_abstract_name(format!("f3note-{}", &digest[..16]))
}

pub enum Role {
    /// This process owns the socket and should open the window.
    Primary(UnixListener),
    /// Another instance is already running and has been handed the files.
    Delegated,
}

/// Try to become the single instance.
///
/// On success the caller owns the listener and should call [`listen`]. If
/// another instance already holds the socket, `paths` are sent to it and this
/// process should exit.
pub fn claim(paths: &[PathBuf]) -> std::io::Result<Role> {
    let addr = address()?;
    match UnixListener::bind_addr(&addr) {
        Ok(listener) => {
            listener.set_nonblocking(true)?;
            Ok(Role::Primary(listener))
        }
        Err(_) => match UnixStream::connect_addr(&addr) {
            Ok(mut stream) => {
                for path in paths {
                    // One absolute path per line. A newline in a filename would
                    // corrupt the framing, so such a path is skipped rather
                    // than sent: it is not worth a length-prefixed protocol for
                    // a case that cannot occur on any sane filesystem.
                    let text = path.to_string_lossy();
                    if text.contains('\n') {
                        eprintln!("f3note: skipping path containing a newline: {text}");
                        continue;
                    }
                    writeln!(stream, "{text}")?;
                }
                stream.flush()?;
                Ok(Role::Delegated)
            }
            // Bound but unreachable should be impossible with an abstract
            // address. Treating it as "we are primary" would risk two
            // processes sharing a state directory, so it is reported instead.
            Err(e) => Err(e),
        },
    }
}

/// Watch the listener and hand each delivered path to `on_open`.
///
/// The socket is watched through the GLib main loop rather than from a thread
/// blocking on `accept`. That keeps an idle editor at genuinely zero work, and
/// — more importantly — means the callback runs on the main thread, where it
/// is safe to touch widgets directly instead of marshalling across a channel.
///
/// The descriptor is duplicated for the watch so `gio::Socket` can own one
/// copy while the standard listener keeps another to accept on.
pub fn listen<F>(listener: UnixListener, on_open: F) -> std::io::Result<()>
where
    F: Fn(Vec<PathBuf>) + 'static,
{
    use gtk::glib;
    use std::os::fd::AsFd;

    let watch_fd = listener.as_fd().try_clone_to_owned()?;
    let socket =
        gtk::gio::Socket::from_fd(watch_fd).map_err(|e| std::io::Error::other(e.to_string()))?;

    // Disambiguated explicitly: gio implements create_source for both
    // SocketExtManual and DatagramBasedExtManual, and a plain method call is
    // ambiguous between them.
    let source = gtk::gio::prelude::SocketExtManual::create_source(
        &socket,
        glib::IOCondition::IN,
        None::<&gtk::gio::Cancellable>,
        Some("f3note-ipc"),
        glib::Priority::DEFAULT,
        move |_, _| {
            // Drain everything pending: several `f3note x.txt` invocations can
            // land between two turns of the main loop.
            while let Ok((stream, _)) = listener.accept() {
                let mut paths = Vec::new();
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let line = line.trim();
                    if !line.is_empty() {
                        paths.push(PathBuf::from(line));
                    }
                }
                if !paths.is_empty() {
                    on_open(paths);
                }
            }
            glib::ControlFlow::Continue
        },
    );
    source.attach(Some(&glib::MainContext::default()));
    // The socket must outlive the source it produced.
    std::mem::forget(socket);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_is_scoped_to_the_state_directory() {
        std::env::set_var("XDG_STATE_HOME", "/tmp/one");
        let a = address().unwrap();
        std::env::set_var("XDG_STATE_HOME", "/tmp/two");
        let b = address().unwrap();
        std::env::remove_var("XDG_STATE_HOME");

        assert!(a.as_abstract_name().is_some());
        assert_ne!(
            a.as_abstract_name(),
            b.as_abstract_name(),
            "separate state directories must be separate instances"
        );
    }

    #[test]
    fn a_second_claim_delegates_instead_of_becoming_primary() {
        // Bind directly rather than going through `claim`, so this test does
        // not depend on whether a real f3note happens to be running.
        let name = format!("f3note-test-{}", std::process::id());
        let addr = SocketAddr::from_abstract_name(&name).unwrap();
        let listener = UnixListener::bind_addr(&addr).unwrap();

        // A second bind to the same abstract name must fail, which is the
        // property the whole mechanism rests on.
        assert!(UnixListener::bind_addr(&addr).is_err());

        // And connecting to it must work, which is how the second process
        // hands over its files.
        let mut client = UnixStream::connect_addr(&addr).unwrap();
        writeln!(client, "/tmp/handed-over.txt").unwrap();
        client.flush().unwrap();
        drop(client);

        let (stream, _) = listener.accept().unwrap();
        let received: Vec<String> = BufReader::new(stream)
            .lines()
            .map_while(Result::ok)
            .collect();
        assert_eq!(received, vec!["/tmp/handed-over.txt"]);
    }

    #[test]
    fn an_abstract_address_leaves_nothing_behind_when_its_owner_dies() {
        let name = format!("f3note-gone-{}", std::process::id());
        let addr = SocketAddr::from_abstract_name(&name).unwrap();
        {
            let _listener = UnixListener::bind_addr(&addr).unwrap();
            assert!(UnixListener::bind_addr(&addr).is_err());
        }
        // The listener is dropped, and with it the address. No unlink, no
        // stale-socket check, no race.
        assert!(
            UnixListener::bind_addr(&addr).is_ok(),
            "an abstract address must be reusable once released"
        );
    }
}
