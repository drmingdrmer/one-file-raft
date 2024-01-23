use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;

use itertools::Itertools;

use crate::Log;
use crate::Metrics;
use crate::Progress;
use crate::Reply;
use crate::Request;
use crate::Vote;

pub struct Display<'a, T> {
    v: &'a T,
}

pub(crate) trait DisplayExt {
    fn display(&self) -> Display<Self>
    where Self: Sized {
        Display { v: self }
    }
}

impl<T> DisplayExt for T {}

impl<'a, T, E> fmt::Display for Display<'a, Result<T, E>>
where
    T: fmt::Display,
    E: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.v {
            Ok(v) => write!(f, "Ok({})", v),
            Err(e) => write!(f, "Err({})", e),
        }
    }
}

impl<'a, T> fmt::Display for Display<'a, BTreeSet<T>>
where T: fmt::Display
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{")?;
        let s = self.v.iter().map(|x| x.to_string()).join(", ");
        write!(f, "{}", s)?;
        write!(f, "}}")
    }
}
impl<'a, K, V> fmt::Display for Display<'a, BTreeMap<K, V>>
where
    K: fmt::Display,
    V: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{")?;
        let s = self.v.iter().map(|(k, v)| format!("{k}={v}")).join(", ");
        write!(f, "{}", s)?;
        write!(f, "}}")
    }
}

impl<'a, T> fmt::Display for Display<'a, Vec<BTreeSet<T>>>
where T: fmt::Display
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[")?;
        let s = self.v.iter().map(|x| x.display().to_string()).join(", ");
        write!(f, "{}", s)?;
        write!(f, "]")
    }
}

impl fmt::Display for Vote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = if self.committed.is_some() {
            "COMMITTED"
        } else {
            "uncommitted"
        };
        write!(f, "T{}-{c}-{}", self.term, self.voted_for)
    }
}

impl fmt::Display for Log {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Log({}", self.log_id)?;
        if self.data.is_none() && self.config.is_none() {
            write!(f, ", noop)")?;
        }
        if let Some(ref d) = self.data {
            write!(f, ", data:{}", d)?;
        }
        if let Some(ref c) = self.config {
            write!(f, ", members:{}", c.display())?;
        }
        write!(f, ")")
    }
}

impl fmt::Display for Progress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "([{}, {}), {})",
            self.acked,
            self.len,
            if self.ready.is_some() {
                "READY"
            } else {
                "busy"
            }
        )
    }
}

impl fmt::Display for Metrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Metrics(vote:{}, last:{}, commit:{}", self.vote, self.last_log, self.commit)?;
        write!(f, ", config:{}", self.config.display())?;
        if let Some(ref progresses) = self.progresses {
            write!(f, ", progresses:{{")?;
            let ps = progresses.iter().map(|(id, p)| format!("N{id}={p}")).join(", ");
            write!(f, " {ps}",)?;
            write!(f, " }}")?;
        }
        write!(f, ")")
    }
}

impl fmt::Display for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(vote:{}, last:{}", self.vote, self.last_log_id,)?;
        write!(f, " commit:{}", self.commit)?;
        write!(f, " logs:{}<{})", self.prev, self.logs.iter().join(","))
    }
}

impl fmt::Display for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let granted = if self.granted { "granted" } else { "denied" };
        let log = match self.log {
            Ok(acked) => {
                format!("Ack({})", acked)
            }
            Err(end) => {
                format!("Conflict({})", end)
            }
        };
        write!(f, "({granted}, vote:{}, {})", self.vote, log)
    }
}
