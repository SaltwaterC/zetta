//! The Windows half's side of the relay: one agent connection per connection
//! inside the distribution.
//!
//! Portable, with the agent behind `connect`, so the whole round trip is
//! tested against the real relay on every host; only opening the pipe is
//! Windows code (`launcher.rs`).

use std::{
    collections::HashMap,
    io::{self, Read, Write},
    sync::{Arc, Mutex, mpsc},
    thread,
};

use crate::protocol::{self, Message, read_agent_frame};

/// Answers the relay's connections from agents `connect` opens, until the
/// relay's stream ends.
pub fn run<W, C, S>(mut input: impl Read, output: W, connect: C) -> io::Result<()>
where
    W: Write + Send + 'static,
    C: Fn() -> io::Result<S> + Send + Sync + 'static,
    S: Read + Write,
{
    let output = Arc::new(Mutex::new(output));
    let connect = Arc::new(connect);
    let mut workers: HashMap<u32, mpsc::Sender<Vec<u8>>> = HashMap::new();
    while let Some(message) = Message::read_from(&mut input)? {
        match message {
            Message::Open(id) => {
                let (requests, received) = mpsc::channel();
                let spawned = thread::Builder::new()
                    .name(format!("wslx-agent-{id}"))
                    .spawn({
                        let output = Arc::clone(&output);
                        let connect = Arc::clone(&connect);
                        move || serve_connection(id, &*connect, &received, &output)
                    });
                if spawned.is_ok() {
                    workers.insert(id, requests);
                } else {
                    protocol::send(&output, &Message::Close(id))?;
                }
            }
            Message::Request(id, frame) => {
                // A worker that has gone already sent `Close`; the request
                // crossed it and has nobody to answer it.
                if workers
                    .get(&id)
                    .is_some_and(|worker| worker.send(frame).is_err())
                {
                    workers.remove(&id);
                }
            }
            Message::Close(id) => {
                workers.remove(&id);
            }
            Message::Reply(..) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the relay sent a message only wslx.exe sends",
                ));
            }
        }
    }
    Ok(())
}

fn serve_connection<S: Read + Write>(
    id: u32,
    connect: &dyn Fn() -> io::Result<S>,
    requests: &mpsc::Receiver<Vec<u8>>,
    output: &Mutex<impl Write>,
) {
    let closed_by_relay = match connect() {
        Ok(mut agent) => relay_requests(id, &mut agent, requests, output),
        Err(_) => false,
    };
    if !closed_by_relay {
        let _ = protocol::send(output, &Message::Close(id));
    }
}

/// Carries requests to the agent and replies back until either side is done.
/// Returns whether the relay ended it, in which case it is not told.
fn relay_requests(
    id: u32,
    agent: &mut (impl Read + Write),
    requests: &mpsc::Receiver<Vec<u8>>,
    output: &Mutex<impl Write>,
) -> bool {
    loop {
        let Ok(request) = requests.recv() else {
            return true;
        };
        if agent
            .write_all(&request)
            .and_then(|()| agent.flush())
            .is_err()
        {
            return false;
        }
        let Ok(Some(reply)) = read_agent_frame(agent) else {
            return false;
        };
        if protocol::send(output, &Message::Reply(id, reply)).is_err() {
            // The relay's stream is gone; there is nobody left to tell.
            return true;
        }
    }
}
