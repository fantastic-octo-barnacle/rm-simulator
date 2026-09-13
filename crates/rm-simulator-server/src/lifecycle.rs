// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Owned listener threads, including connections still waiting for a handshake.
use std::io;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Transport-owned cancellation; the simulation worker never touches UDP sockets.
pub(crate) enum ConnectionStop {
    /// The connection owns its socket, closed by shutting the stream down.
    Tcp(TcpStream),
    /// A GNS connection is closed by its own worker, so the stop signal is the
    /// whole cancellation.
    Worker(Stop),
}
impl ConnectionStop {
    /// Cancel the connection in the way its transport supports. A worker stop
    /// always reports success.
    pub(crate) fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.shutdown(how),
            Self::Worker(stop) => {
                stop.request();
                Ok(())
            }
        }
    }
}

/// A shared shutdown flag. Clones signal the same flag, so any holder can stop
/// every waiter.
#[derive(Clone, Default)]
pub(crate) struct Stop(Arc<(Mutex<bool>, Condvar)>);
impl Stop {
    /// Set the flag and wake every waiter. Idempotent.
    pub(crate) fn request(&self) {
        let (stopped, changed) = &*self.0;
        *stopped.lock().unwrap_or_else(|p| p.into_inner()) = true;
        changed.notify_all();
    }

    /// Sleep until the next work interval, or wake immediately on shutdown.
    pub(crate) fn wait(&self, interval: Duration) -> bool {
        let (stopped, changed) = &*self.0;
        let stopped = stopped.lock().unwrap_or_else(|p| p.into_inner());
        if *stopped || interval.is_zero() {
            return *stopped;
        }
        let (stopped, _) = changed
            .wait_timeout_while(stopped, interval, |stopped| !*stopped)
            .unwrap_or_else(|p| p.into_inner());
        *stopped
    }
}

/// A bound listener and the worker that accepts for it. Dropping it stops and
/// joins the worker.
pub(crate) struct Listener {
    /// Address the socket is actually bound to.
    pub(crate) local_addr: SocketAddr,
    stop: Stop,
    worker: Option<JoinHandle<()>>,
}
impl Listener {
    /// Start the accepting worker under the given thread name.
    ///
    /// The worker polls the listener in nonblocking mode, moves each accepted
    /// stream back to blocking mode and serves it on its own thread. `serve`
    /// runs per connection and must not use the listener. Completed connection
    /// workers are reaped as the loop runs. Shutdown stops accepting within one
    /// 10 ms poll, shuts every live stream down and joins all of them,
    /// including connections still waiting for a handshake.
    pub(crate) fn start(
        listener: TcpListener,
        stop: Stop,
        name: &'static str,
        serve: impl Fn(TcpStream) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let local_addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let stopping = stop.clone();
        let serve = Arc::new(serve);
        let worker = thread::Builder::new().name(name.into()).spawn(move || {
            let mut connections: Vec<(TcpStream, JoinHandle<()>)> = Vec::new();
            while !stopping.wait(Duration::ZERO) {
                // Reap completed workers so a long-running server keeps only
                // active connections. Shutdown also includes incomplete hellos.
                let mut index = 0;
                while index < connections.len() {
                    if connections[index].1.is_finished() {
                        let (_, worker) = connections.swap_remove(index);
                        let _ = worker.join();
                    } else {
                        index += 1;
                    }
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        // Accepted sockets inherit nonblocking mode on some
                        // platforms; only the listener uses polling.
                        if let Err(error) = stream.set_nonblocking(false) {
                            eprintln!("{name}: configuring connection: {error}");
                            continue;
                        }
                        let tracked = match stream.try_clone() {
                            Ok(tracked) => tracked,
                            Err(error) => {
                                eprintln!("{name}: tracking connection: {error}");
                                continue;
                            }
                        };
                        let serve = serve.clone();
                        match thread::Builder::new()
                            .name(format!("{name}-peer"))
                            .spawn(move || serve(stream))
                        {
                            Ok(worker) => connections.push((tracked, worker)),
                            Err(error) => eprintln!("{name}: starting connection: {error}"),
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        stopping.wait(Duration::from_millis(10));
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        eprintln!("{name}: accept failed: {error}");
                        stopping.request();
                    }
                }
            }
            // Release the port before waiting for handlers to finish.
            drop(listener);
            for (stream, _) in &connections {
                let _ = stream.shutdown(Shutdown::Both);
            }
            for (_, worker) in connections {
                let _ = worker.join();
            }
        })?;
        Ok(Self {
            local_addr,
            stop,
            worker: Some(worker),
        })
    }

    /// Stop accepting, close live connections and join the worker. Idempotent.
    pub(crate) fn shutdown(&mut self) {
        self.stop.request();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Listener {
    /// Stop accepting and join the worker.
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::mpsc;

    #[test]
    fn shutdown_interrupts_idle_handlers_and_joins_them() {
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let stop = Stop::default();
        let (started_tx, started_rx) = mpsc::channel();
        let (ended_tx, ended_rx) = mpsc::channel();
        let mut listener = Listener::start(socket, stop, "test-listener", move |mut stream| {
            started_tx.send(()).unwrap();
            let _ = stream.read(&mut [0; 1]);
            ended_tx.send(()).unwrap();
        })
        .unwrap();
        let address = listener.local_addr;
        let _client = TcpStream::connect(address).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        listener.shutdown();
        ended_rx.try_recv().unwrap();
        let _rebound = TcpListener::bind(address).unwrap();
        listener.shutdown();
    }
}
