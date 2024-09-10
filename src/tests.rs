#![allow(clippy::let_underscore_future)]

use std::collections::BTreeSet;

use log::debug;
use log::info;
use mpsc::UnboundedSender;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;

use crate::Event;
use crate::LeaderId;
use crate::Log;
use crate::Metrics;
use crate::Net;
use crate::Raft;
use crate::Store;

#[tokio::test(flavor = "current_thread")]
async fn test_raft() {
    env_logger::builder().filter_level(log::LevelFilter::Debug).init();

    fn btset(
        config: impl IntoIterator<Item = impl IntoIterator<Item = u64>>,
    ) -> Vec<BTreeSet<u64>> {
        config.into_iter().map(|x| x.into_iter().collect()).collect()
    }

    let sto123 = || Store::new(btset([[1, 2, 3]]));

    let (tx1, rx1) = mpsc::unbounded_channel();
    let (tx2, rx2) = mpsc::unbounded_channel();
    let (tx3, rx3) = mpsc::unbounded_channel();
    let (tx4, rx4) = mpsc::unbounded_channel();

    let net = || {
        let mut r = Net::default();
        r.targets.insert(1, tx1.clone());
        r.targets.insert(2, tx2.clone());
        r.targets.insert(3, tx3.clone());
        r.targets.insert(4, tx4.clone());
        r
    };

    let n1 = Raft::new(1, sto123(), net(), rx1);
    let n2 = Raft::new(2, sto123(), net(), rx2);
    let n3 = Raft::new(3, sto123(), net(), rx3);
    let n4 = Raft::new(4, Store::new(btset([[]])), net(), rx4);

    let h1 = tokio::spawn(n1.run());
    let h2 = tokio::spawn(n2.run());
    let h3 = tokio::spawn(n3.run());
    let h4 = tokio::spawn(n4.run());

    async fn func<T>(
        raft_tx: &UnboundedSender<(u64, Event)>,
        f: impl FnOnce(&mut Raft) -> T + Send + 'static,
    ) -> T
    where
        T: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let call_and_send = |raft: &mut Raft| {
            let ret = f(raft);
            let _ = tx.send(ret);
        };
        raft_tx.send((0, Event::Func(Box::new(call_and_send)))).unwrap();
        rx.await.unwrap()
    }

    async fn metrics(raft_tx: &UnboundedSender<(u64, Event)>) -> watch::Receiver<Metrics> {
        func(raft_tx, |raft| raft.metrics.subscribe()).await
    }

    async fn elect(raft_tx: &UnboundedSender<(u64, Event)>, id: u64) {
        func(raft_tx, |raft| raft.elect()).await;

        let mut mtx = metrics(raft_tx).await;

        loop {
            {
                let mm = mtx.borrow();
                if mm.vote.voted_for == LeaderId(id) && mm.vote.committed.is_some() {
                    info!("=== Leader established {}", mm.vote);
                    break;
                }
            }
            mtx.changed().await.unwrap();
        }
    }

    /// Write a log to a raft node
    async fn write(raft_tx: &UnboundedSender<(u64, Event)>, log: Log) -> String {
        let (tx, rx) = oneshot::channel();
        func(raft_tx, |raft| raft.write(tx, log)).await;

        let got = rx.await.unwrap();
        debug!("got: {:?}", got);
        got
    }

    elect(&tx1, 1).await;
    write(&tx1, Log::new(Some(s("x=1")), None)).await;
    elect(&tx2, 2).await;
    write(&tx2, Log::new(None, Some(btset([vec![1, 2, 3], vec![1, 2, 3, 4]])))).await;
    write(&tx2, Log::new(None, Some(btset([vec![1, 2, 3, 4]])))).await;

    let metrics = metrics(&tx2).await;
    info!("=== {}", metrics.borrow().clone());

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let _ = h1;
    let _ = h2;
    let _ = h3;
    let _ = h4;
}

fn s(x: impl ToString) -> String {
    x.to_string()
}
