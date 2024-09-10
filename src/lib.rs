#![doc = include_str!("../README.md")]

//!
//! - Read the [Tutorial](`crate::docs::tutorial`);
//! - Read the [Tutorial-cn](`crate::docs::tutorial_cn`);
//!
//! Features:
//!
//! - [x] Election(`Raft::elect()`)
//! - [x] Log replication(`Raft::handle_replicate_req()`)
//! - [x] Commit
//! - [x] Write application data(`Raft::write()`)
//! - [x] Membership store(`Store::configs`).
//! - [x] Membership change: joint consensus.
//! - [x] Event loop model(main loop: `Raft::run()`).
//! - [x] Pseudo network simulated by mpsc channels(`Net`).
//! - [x] Pseudo Log store simulated by in-memory store(`Store`).
//! - [x] Raft log data is a simple `String`
//! - [x] Metrics
//!
//! Not yet implemented:
//! - [ ] State machine(`Raft::commit()` is a no-op entry)
//! - [ ] Log compaction
//! - [ ] Log purge
//! - [ ] Heartbeat
//! - [ ] Leader lease
//! - [ ] Linearizable read
//!
//! Implementation details:
//! - [x] Membership config takes effect once appended(not on-applied).
//! - [x] Standalone Leader, it has to check vote when accessing local store.
//! - [x] Leader access store directly(not via RPC).
//! - [ ] Append log when vote?

#![feature(map_try_insert)]

mod display;
pub mod docs;
#[cfg(test)]
mod tests;

use std::cmp::max;
use std::cmp::min;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

use derivative::Derivative;
use derive_more::Display;
use derive_new::new as New;
use itertools::Itertools;
use log::debug;
use log::error;
use log::info;
use log::trace;
use mpsc::UnboundedReceiver;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;

use crate::display::DisplayExt;

#[derive(Debug, Clone, Default, Copy, PartialEq, Eq, Hash, Display)]
#[display(fmt = "L({})", _0)]
pub struct LeaderId(pub u64);

impl PartialOrd for LeaderId {
    fn partial_cmp(&self, b: &Self) -> Option<Ordering> {
        [None, Some(Ordering::Equal)][(self.0 == b.0) as usize]
    }
}

#[derive(Debug, Clone, Default, Copy, PartialEq, Eq, PartialOrd, Hash)]
#[derive(New)]
pub struct Vote {
    pub term: u64,
    pub committed: Option<()>,
    pub voted_for: LeaderId,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Display)]
#[display(fmt = "T{}-{}", term, index)]
pub struct LogId {
    term: u64,
    index: u64,
}

#[derive(Debug, Clone, Default, New)]
pub struct Log {
    #[new(default)]
    pub log_id: LogId,
    pub data: Option<String>,
    pub config: Option<Vec<BTreeSet<u64>>>,
}

#[derive(Debug, Default)]
pub struct Net {
    pub targets: BTreeMap<u64, mpsc::UnboundedSender<(u64, Event)>>,
}

impl Net {
    fn send(&mut self, from: u64, target: u64, ev: Event) {
        trace!("N{} send --> N{} {}", from, target, ev);
        let tx = self.targets.get(&target).unwrap();
        tx.send((from, ev)).unwrap();
    }
}

#[derive(Debug)]
pub struct Request {
    vote: Vote,
    last_log_id: LogId,

    prev: LogId,
    logs: Vec<Log>,
    commit: u64,
}

#[derive(Debug)]
pub struct Reply {
    granted: bool,
    vote: Vote,
    log: Result<LogId, u64>,
}

#[derive(Display)]
pub enum Event {
    #[display(fmt = "Request({})", _0)]
    Request(Request),
    #[display(fmt = "Reply({})", _0)]
    Reply(Reply),
    #[display(fmt = "Write({})", _1)]
    Write(oneshot::Sender<String>, Log),
    #[display(fmt = "Func")]
    Func(Box<dyn FnOnce(&mut Raft) + Send + 'static>),
}

#[derive(Debug, Clone, Copy, Derivative, PartialEq, Eq, New)]
#[derivative(Default)]
pub struct Progress {
    acked: LogId,
    len: u64,
    /// It is a token to indicate if it can send an RPC, e.g., there is no inflight RPC sending.
    /// It is set to `None` when an RPC is sent, and set to `Some(())` when the RPC is finished.
    #[derivative(Default(value = "Some(())"))]
    ready: Option<()>,
}

pub struct Leading {
    granted_by: BTreeSet<u64>,
    progresses: BTreeMap<u64, Progress>,
    log_index_range: (u64, u64),
}

#[derive(Debug, Default, Clone, PartialEq, Eq, New)]
pub struct Metrics {
    pub vote: Vote,
    pub last_log: LogId,
    pub commit: u64,
    pub config: Vec<BTreeSet<u64>>,
    pub progresses: Option<BTreeMap<u64, Progress>>,
}

#[derive(Debug, Default)]
pub struct Store {
    id: u64,
    vote: Vote,
    configs: BTreeMap<u64, Vec<BTreeSet<u64>>>,
    replies: BTreeMap<u64, oneshot::Sender<String>>,
    logs: Vec<Log>,
}

impl Store {
    pub fn new(membership: Vec<BTreeSet<u64>>) -> Self {
        let mut configs = BTreeMap::new();
        let vote = Vote::default();
        configs.insert(0, membership);
        let replies = BTreeMap::new();
        Store { id: 0, vote, configs, replies, logs: vec![Log::default()] }
    }

    pub fn config(&self) -> &Vec<BTreeSet<u64>> {
        self.configs.values().last().unwrap()
    }

    fn last(&self) -> LogId {
        self.logs.last().map(|x| x.log_id).unwrap_or_default()
    }

    fn truncate(&mut self, log_id: LogId) {
        debug!("truncate: {}", log_id);
        self.replies.retain(|&x, _| x < log_id.index);
        self.configs.retain(|&x, _| x < log_id.index);
        self.logs.truncate(log_id.index as usize);
    }

    fn append(&mut self, logs: Vec<Log>) {
        if logs.is_empty() {
            return;
        }
        debug!("N{} append: [{}]", self.id, logs.iter().join(", "));
        for log in logs {
            if let Some(x) = self.get_log_id(log.log_id.index) {
                if x != log.log_id {
                    self.truncate(x);
                } else {
                    continue;
                }
            }
            if let Some(ref membership) = log.config {
                self.configs.insert(log.log_id.index, membership.clone());
            }
            self.logs.push(log);
        }
    }

    fn get_log_id(&self, rel_index: u64) -> Option<LogId> {
        self.logs.get(rel_index as usize).map(|x| x.log_id)
    }

    fn read_logs(&self, i: u64, n: u64) -> Vec<Log> {
        if n == 0 {
            return vec![];
        }

        let logs: Vec<_> = self.logs[i as usize..].iter().take(n as usize).cloned().collect();
        debug!("N{} read_logs: [{i},+{n})={}", self.id, logs.iter().join(","));
        logs
    }
}

pub struct Raft {
    pub id: u64,
    pub leading: Option<Leading>,
    pub commit: u64,
    pub net: Net,
    pub sto: Store,
    pub metrics: watch::Sender<Metrics>,
    pub rx: UnboundedReceiver<(u64, Event)>,
}

impl Raft {
    pub fn new(id: u64, mut sto: Store, net: Net, rx: UnboundedReceiver<(u64, Event)>) -> Self {
        let (metrics, _) = watch::channel(Metrics::default());
        sto.id = id;
        Raft { id, leading: None, commit: 0, net, sto, metrics, rx }
    }

    pub async fn run(mut self) -> Result<(), anyhow::Error> {
        loop {
            let mem = self.sto.config().clone();
            #[allow(clippy::useless_asref)]
            let ps = self.leading.as_ref().map(|x| x.progresses.clone());
            let m = Metrics::new(self.sto.vote, self.sto.last(), self.commit, mem, ps);
            self.metrics.send_replace(m);

            let (from, ev) = self.rx.recv().await.ok_or(anyhow::anyhow!("closed"))?;
            debug!("N{} recv <-- N{} {}", self.id, from, ev);
            match ev {
                Event::Request(req) => {
                    let reply = self.handle_replicate_req(req);
                    self.net.send(self.id, from, Event::Reply(reply));
                }
                Event::Reply(reply) => {
                    self.handle_replicate_reply(from, reply);
                }
                Event::Write(tx, log) => {
                    let res = self.write(tx, log.clone());
                    if res.is_none() {
                        error!("N{} leader can not write : {}", self.id, log);
                    }
                }
                Event::Func(f) => {
                    f(&mut self);
                }
            }
        }
    }

    pub fn elect(&mut self) {
        self.sto.vote = Vote::new(self.sto.vote.term + 1, None, LeaderId(self.id));

        let noop_index = self.sto.last().index + 1;
        let config = self.sto.config().clone();
        let p = Progress::new(LogId::default(), noop_index, Some(()));

        debug!("N{} elect: ids: {}", self.id, node_ids(&config).join(","));

        self.leading = Some(Leading {
            granted_by: BTreeSet::new(),
            progresses: node_ids(&config).map(|id| (id, p)).collect(),
            log_index_range: (noop_index, noop_index),
        });

        node_ids(&config).for_each(|id| self.send_if_idle(id, 0).unwrap_or(()));
    }

    pub fn write(&mut self, tx: oneshot::Sender<String>, mut log: Log) -> Option<LogId> {
        self.sto.vote.committed?;
        let l = self.leading.as_mut()?;

        let log_id = LogId { term: self.sto.vote.term, index: l.log_index_range.1 };
        l.log_index_range.1 += 1;
        log.log_id = log_id;

        if let Some(ref membership) = log.config {
            if self.sto.configs.keys().last().copied().unwrap() > self.commit {
                panic!("N{} can write membership: {} before committing the previous", self.id, log);
            }
            let ids = node_ids(membership);
            l.progresses = ids.map(|x| (x, l.progresses.remove(&x).unwrap_or_default())).collect();
            info!("N{} rebuild progresses: {}", self.id, l.progresses.display());
        }
        self.sto.replies.insert(log_id.index, tx);
        self.sto.append(vec![log]);

        // Mark it as sending, so that it won't be sent again.
        l.progresses.insert(self.id, Progress::new(log_id, log_id.index, None));

        node_ids(self.sto.config()).for_each(|id| self.send_if_idle(id, 10).unwrap_or(()));
        Some(log_id)
    }

    pub fn handle_replicate_req(&mut self, req: Request) -> Reply {
        let my_last = self.sto.last();
        let (is_granted, vote) = self.check_vote(req.vote);
        let is_upto_date = req.last_log_id >= my_last;

        let req_last = req.logs.last().map(|x| x.log_id).unwrap_or(req.prev);

        if is_granted && is_upto_date {
            let log = if self.sto.get_log_id(req.prev.index) == Some(req.prev) {
                self.sto.append(req.logs);
                self.commit(min(req.commit, req_last.index));
                Ok(req_last)
            } else {
                self.sto.truncate(req.prev);
                Err(req.prev.index)
            };

            Reply { granted: true, vote, log }
        } else {
            Reply { granted: false, vote, log: Err(my_last.index + 1) }
        }
    }

    pub fn handle_replicate_reply(&mut self, target: u64, reply: Reply) -> Option<Leading> {
        let l = self.leading.as_mut()?;
        let v = self.sto.vote;

        let is_same_leader = reply.vote.term == v.term && reply.vote.voted_for == v.voted_for;

        // 0. Set a replication channel to `ready`, once a reply is received.
        if is_same_leader {
            assert!(l.progresses[&target].ready.is_none());
            l.progresses.get_mut(&target).unwrap().ready = Some(());
        }

        if reply.granted && is_same_leader {
            // 1. Vote is granted, means that Log replication privilege is acquired.
            if v.committed.is_none() {
                debug!("N{} is granted by: N{}", self.id, target);
                l.granted_by.insert(target);

                if is_quorum(self.sto.config(), &l.granted_by) {
                    self.sto.vote.committed = Some(());
                    info!("N{} Leader established: {}", self.id, self.sto.vote);

                    let (tx, _rx) = oneshot::channel();
                    self.net.send(self.id, self.id, Event::Write(tx, Log::default()));
                }
            }

            let p = l.progresses.get_mut(&target).unwrap();

            // 2. Update the log replication progress

            *p = match reply.log {
                Ok(acked) => Progress::new(acked, max(p.len, acked.index + 1), Some(())),
                Err(len) => Progress::new(p.acked, min(p.len, len), Some(())),
            };
            debug!("N{} progress N{target}={}", self.id, p);

            // 3. Update committed index

            let (noop_index, len) = l.log_index_range;
            let acked = p.acked.index;

            let acked_desc = l.progresses.values().map(|p| p.acked).sorted().rev();
            let mut max_committed = acked_desc.filter(|acked| {
                let greater_equal = l.progresses.iter().filter(|(_id, p)| p.acked >= *acked);
                acked.index >= noop_index
                    && is_quorum(self.sto.config(), greater_equal.map(|(id, _)| id))
            });

            if let Some(log_id) = max_committed.next() {
                self.commit(log_id.index)
            }

            // 4. Keep sending
            if len - 1 > acked {
                self.send_if_idle(target, len - 1 - acked);
            }
        } else {
            self.check_vote(reply.vote);
        }
        None
    }

    pub fn send_if_idle(&mut self, target: u64, n: u64) -> Option<()> {
        let l = self.leading.as_mut().unwrap();

        let p = l.progresses.get_mut(&target).unwrap();
        p.ready.take()?;

        let prev = (p.acked.index + p.len) / 2;

        let req = Request {
            vote: self.sto.vote,
            last_log_id: self.sto.last(),

            prev: self.sto.get_log_id(prev).unwrap(),
            logs: self.sto.read_logs(prev + 1, n),

            commit: self.commit,
        };

        self.net.send(self.id, target, Event::Request(req));
        Some(())
    }

    fn commit(&mut self, i: u64) {
        if i > self.commit {
            info!("N{} commit: {i}: {}", self.id, self.sto.logs[i as usize]);
            self.commit = i;
            let right = self.sto.replies.split_off(&(i + 1));
            for (i, tx) in std::mem::replace(&mut self.sto.replies, right).into_iter() {
                let _ = tx.send(format!("{}", i));
            }
        }
    }

    pub fn check_vote(&mut self, vote: Vote) -> (bool, Vote) {
        trace!("N{} check_vote: my:{}, {}", self.id, self.sto.vote, vote);

        if vote > self.sto.vote {
            info!("N{} update_vote: {} --> {}", self.id, self.sto.vote, vote);
            self.sto.vote = vote;

            if vote.voted_for != LeaderId(self.id) && self.leading.is_some() {
                info!("N{} Leading quit: vote:{}", self.id, self.sto.vote);
                self.leading = None;
            }
        }

        trace!("check_vote: ret: {}", self.sto.vote);
        (vote == self.sto.vote, self.sto.vote)
    }
}

pub fn is_quorum<'a>(config: &[BTreeSet<u64>], granted: impl IntoIterator<Item = &'a u64>) -> bool {
    let granted = granted.into_iter().copied().collect::<BTreeSet<_>>();
    for group in config {
        if group.intersection(&granted).count() <= group.len() / 2 {
            return false;
        }
    }
    true
}

pub fn node_ids(config: &[BTreeSet<u64>]) -> impl Iterator<Item = u64> + 'static {
    config.iter().flat_map(|x| x.iter().copied()).collect::<BTreeSet<_>>().into_iter()
}
