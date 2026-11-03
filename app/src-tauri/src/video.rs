//! Video previews: background readers keep the file open on the iPhone and
//! read ahead of the play position into memory, so the player's Range
//! requests are answered from RAM instead of a fresh device read each.
//! Several readers: a slow iPhone (~2 MB/s per connection) gives about twice
//! the throughput with three.

use async_trait::async_trait;
use photoport_core::source::afc::{AfcSource, BlockPlan};
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

const BLOCK: usize = 1024 * 1024;
const READERS: usize = 3;
/// Read-ahead past the block the player last asked for (~5 s of 4K HEVC).
const AHEAD: u64 = 32;
const MAX_BLOCKS: usize = 160;
const MAX_RESPONSE: usize = 8 * 1024 * 1024;
const WAIT_FOR_BLOCK: Duration = Duration::from_secs(20);
/// Reader stops once the preview has been closed this long.
const IDLE: Duration = Duration::from_secs(30);

pub struct VideoStream {
    pub path: String,
    size: u64,
    inner: Mutex<Inner>,
    tick: watch::Sender<u64>,
}

struct Inner {
    blocks: BTreeMap<u64, Arc<Vec<u8>>>,
    /// Blocks a reader is fetching right now.
    inflight: HashSet<u64>,
    readers: usize,
    target: u64,
    last_request: Instant,
    error: Option<String>,
    closed: bool,
}

impl VideoStream {
    pub fn start(path: String, size: u64) -> Arc<Self> {
        let (tick, _) = watch::channel(0);
        let stream = Arc::new(Self {
            path,
            size,
            inner: Mutex::new(Inner {
                blocks: BTreeMap::new(),
                inflight: HashSet::new(),
                readers: READERS,
                target: 0,
                last_request: Instant::now(),
                error: None,
                closed: false,
            }),
            tick,
        });
        for _ in 0..READERS {
            let s = stream.clone();
            tauri::async_runtime::spawn(async move {
                let mut plan = Plan { rx: s.tick.subscribe(), s: s.clone(), current: None };
                let r = async {
                    let mut src = AfcSource::connect_first().await?;
                    src.read_blocks(&s.path, BLOCK, &mut plan).await
                }
                .await;
                {
                    let mut g = s.inner.lock().unwrap();
                    if let Some(i) = plan.current {
                        g.inflight.remove(&i);
                    }
                    if let Err(e) = r {
                        g.error = Some(e.to_string());
                    }
                    g.readers -= 1;
                    if g.readers == 0 {
                        g.closed = true;
                    }
                }
                s.bump();
            });
        }
        stream
    }

    fn bump(&self) {
        self.tick.send_modify(|v| *v = v.wrapping_add(1));
    }

    pub fn is_closed(&self) -> bool {
        self.inner.lock().unwrap().closed
    }

    pub fn close(&self) {
        self.inner.lock().unwrap().closed = true;
        self.bump();
    }

    /// Bytes from `start` (up to `end`, inclusive), as many as are already
    /// contiguous in memory; waits only for the first block.
    pub async fn read(&self, start: u64, end: u64) -> Result<Vec<u8>, String> {
        let first = start / BLOCK as u64;
        let mut rx = self.tick.subscribe();
        {
            let mut g = self.inner.lock().unwrap();
            g.target = first;
            g.last_request = Instant::now();
        }
        self.bump();
        loop {
            {
                let g = self.inner.lock().unwrap();
                if g.blocks.contains_key(&first) {
                    return Ok(assemble(&g.blocks, start, end));
                }
                if g.closed {
                    return Err(g.error.clone().unwrap_or_else(|| "stream closed".into()));
                }
            }
            if tokio::time::timeout(WAIT_FOR_BLOCK, rx.changed()).await.is_err() {
                return Err("timeout reading video".into());
            }
        }
    }
}

fn assemble(blocks: &BTreeMap<u64, Arc<Vec<u8>>>, start: u64, end: u64) -> Vec<u8> {
    let want = ((end - start + 1) as usize).min(MAX_RESPONSE);
    let mut out = Vec::with_capacity(want.min(2 * BLOCK));
    let mut index = start / BLOCK as u64;
    let mut offset = (start % BLOCK as u64) as usize;
    while out.len() < want {
        let Some(b) = blocks.get(&index) else { break };
        if offset >= b.len() {
            break;
        }
        let take = (b.len() - offset).min(want - out.len());
        out.extend_from_slice(&b[offset..offset + take]);
        if b.len() < BLOCK {
            break; // last block of the file
        }
        index += 1;
        offset = 0;
    }
    out
}

struct Plan {
    s: Arc<VideoStream>,
    rx: watch::Receiver<u64>,
    current: Option<u64>,
}

#[async_trait]
impl BlockPlan for Plan {
    async fn next(&mut self) -> Option<u64> {
        let blocks_total = self.s.size.div_ceil(BLOCK as u64);
        loop {
            {
                let mut g = self.s.inner.lock().unwrap();
                if g.closed {
                    return None;
                }
                if g.last_request.elapsed() > IDLE {
                    g.closed = true;
                    return None;
                }
                let upto = (g.target + AHEAD).min(blocks_total);
                if let Some(i) = (g.target..upto).find(|i| !g.blocks.contains_key(i) && !g.inflight.contains(i)) {
                    g.inflight.insert(i);
                    self.current = Some(i);
                    return Some(i);
                }
            }
            // Read-ahead full: wait for the player to move (or for the idle check).
            let _ = tokio::time::timeout(IDLE, self.rx.changed()).await;
        }
    }

    fn got(&mut self, index: u64, data: Vec<u8>) {
        {
            let mut g = self.s.inner.lock().unwrap();
            g.inflight.remove(&index);
            self.current = None;
            g.blocks.insert(index, Arc::new(data));
            while g.blocks.len() > MAX_BLOCKS {
                // Evict the block farthest from where the player is.
                let target = g.target;
                let far = g.blocks.keys().copied().max_by_key(|k| k.abs_diff(target)).expect("non-empty");
                g.blocks.remove(&far);
            }
        }
        self.s.bump();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assemble_spans_blocks_and_stops_at_gaps() {
        let mut m = BTreeMap::new();
        m.insert(0, Arc::new(vec![1u8; BLOCK]));
        m.insert(1, Arc::new(vec![2u8; BLOCK]));
        m.insert(3, Arc::new(vec![4u8; 10]));
        let out = assemble(&m, BLOCK as u64 - 2, u64::MAX / 2);
        assert_eq!(out.len(), BLOCK + 2, "stops at the missing block 2");
        assert_eq!(&out[..3], &[1, 1, 2]);
        assert_eq!(assemble(&m, 5, 9), vec![1; 5], "honours the end");
        assert_eq!(assemble(&m, 3 * BLOCK as u64 + 4, u64::MAX / 2), vec![4; 6], "short last block");
    }
}
