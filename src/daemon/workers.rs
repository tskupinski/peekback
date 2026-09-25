use std::sync::{Arc, Mutex, mpsc};

use anyhow::{Result, anyhow};

type Job = Box<dyn FnOnce() + Send>;

pub struct Workers(mpsc::SyncSender<Job>);

impl Workers {
    pub fn new(count: usize) -> Self {
        let (send, receive) = mpsc::sync_channel::<Job>(16);
        let receive = Arc::new(Mutex::new(receive));
        for _ in 0..count {
            let receive = receive.clone();
            std::thread::spawn(move || {
                loop {
                    let job = receive.lock().unwrap().recv();
                    match job {
                        Ok(job) => job(),
                        Err(_) => break,
                    }
                }
            });
        }
        Self(send)
    }

    pub fn submit(&self, job: impl FnOnce() + Send + 'static) -> Result<()> {
        self.0.try_send(Box::new(job)).map_err(|_| anyhow!("Viewer is busy; try again"))
    }
}
