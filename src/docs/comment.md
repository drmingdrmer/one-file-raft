## LeaderId

In our demonstration of `one-file-raft`, the concept of `LeaderId`, a `u64` node identifier, emerges as central to understanding leadership within the Raft protocol. This identifier serves two roles: it marks the `Leader` that has won an election, or `Candidate` that is during the election.

`LeaderId` introduces a `PartialOrd` ordering. This ordering aids in decision-making: a node voted for `LeaderId_1` can only vote for `LeaderId_2` if `LeaderId_2 >= LeaderId_1`, excluding other factors like the term.

In the standard Raft protocol, within the same term, a vote can be cast for only one candidate, which means that without considering the term, two different `LeaderIds` cannot be compared. This is the basis for our implementation of `PartialOrd`.

Converting this logic into a `PartialOrd` relationship allows us to later abstract other conditions such as term and RPC types into the `PartialOrd` relationship and encapsulate them within the `Vote` structure. With the `PartialOrd` relationship of `Vote`, a simple comparison can determine the legitimacy of the Leader (accepting or rejecting RPC requests from a certain leader). It also concentrates correctness testing on the implementation of `PartialOrd`, rather than being dispersed throughout the codebase. We will see the powerful effect of this simplified logic later on.

```rust
pub struct LeaderId(pub u64);

impl PartialOrd for LeaderId {
    fn partial_cmp(&self, b: &Self) -> Option<Ordering> {
        [None, Some(Ordering::Equal)][(self.0 == b.0) as usize]
    }
}
```

## Vote

In one-file-raft, we abstract all judgments and operations related to `term` and `voted_for` into a concept called `Vote`:

In standard Raft, every time an external message is received, its legitimacy must be verified:
- 1) When a node receives an elect request, if `req.term > self.term`, it updates its `term` and sets `self.voted_for` to the requesting LeaderId, and replies with OK.
- 2) When a node receives an elect request, if `req.term == self.term`, it replies with Reject unless `req.candidate == self.voted_for`.
- 3) When a node receives an AppendEntries (or InstallSnapshot) request, if `req.term >= self.term`, it updates its `term` and sets `self.voted_for` to LeaderId of the request.
- 4) When a Leader receives a reply to a request, if it finds `self.term < reply.term`, the Leader needs to step down, and updates its `term` and `voted_for`.

> For the logic of standard Raft, you can refer to the implementation of `term` operations in [etcd-raft][etcd-raft-handle-term];

In one-file-raft, **all these logics are put into `Vote`**:

- Examining 1) 2) 4), the condition for updating `(term, voted_for)` can be summarized as:

    ```text
    if (term, voted_for) > (self.term, self.voted_for) {
        self.term = term;
        self.voted_for = voted_for;
    }
    ```

    Here, the type of `voted_for` is the previously defined [LeaderId][docs-LeaderId], and note its `PartialOrd` implementation, which validates condition 2).

- However, 3) is quite special: in an AppendEntries request, **even if the request `term` is the same as the local `term`**, it allows the local `voted_for` to be updated to the request's.
  This is because an AppendEntries request is definitely issued by a Leader, and a Leader must have been granted by a majority, so the local `voted_for` is definitely not granted by the majority, and hence the local `voted_for` can be replaced.
  That is, when the `term` is the same, a `voted_for` approved by a majority can replace a `voted_for` that is not approved by a majority. In one-file-raft, we call information approved by majority (including the elect requests and log replication) as `committed`.

Therefore, all the above updating conditions can be summarized with the definition of [Vote][] in one-file-raft:

```rust
#[derive(PartialOrd)]
pub struct Vote {
    pub term: u64,
    pub committed: Option<()>,
    pub voted_for: LeaderId,
}
```

Notice that `Vote` inherits a `PartialOrd` relationship by sequentially comparing `term`, `committed`, `voted_for`;
thus all operations on `term, voted_for` (`handle_elect`, `handle_elect_reply`, `handle_append_entries`, `handle_append_entries_reply`) can be unified into one logic:

```text
if vote > self.vote {
    self.vote = vote;
}
```

In one-file-raft, the legitimacy of the Leader is processed just by: **updating `vote` to a higher value**.
This also reflects the essential of distributed consistency, which is ordering events, where the order of `vote` defines the sequence of events(logs).
`Vote` can also be seen as a concept of pseudo time in Raft.


## Commit

In distributed consensus systems, the notion of a `commit` is fundamental yet
often underemphasized. In contrast to single-threaded systems where a commit is
straightforward—once a variable is written, it is immediately readable—in the
realm of distributed systems, this simplicity vanishes. Here, a successful write
doesn't always mean that the data will be readable later. This necessitates a
redefinition of what it means to commit. As such, many of the distinctive
challenges faced by distributed systems stem from this nuanced understanding of
commits.

When a value is reliably written and can be consistently read through a defined
sequence of operations, it is deemed to be committed. This highlights that
committing is a critical protocol, a binding agreement between writer and reader
for maintaining data integrity across the system.

> Take, for example, a five-node system that doesn't take node failures into
> consideration. If we focus on a single instance of writing and reading, there
> are several potential arrangements for fulfilling the commit protocol:
>
> - Write to all 5 nodes, then read from any single node during the read phase.
> - Write to any 4 nodes, followed by reading from any 2 nodes.
> - Write to any 3 nodes, with the read phase involving any 3 nodes.
> - Write to any 2 nodes, then read from any 4 nodes.
> - Write to just 1 node, and subsequently read from all 4 nodes during the read phase.

### Definition of Commit in Raft

In Raft, the commitment process is design for **an array of logs**. The protocol
governing the write and read operations for committing a log array stipulates
that:

- The log array must be stored on a majority (over half) of the nodes. This
  guarantees that future readers(Candidate), can see this log array
  by connecting with a majority of the nodes.

- Once a log array becomes visible, it must be selected by any Candidate
  as the definitive record of the system's most recent state changes, taking
  precedence over any other visible logs.

> The Raft consensus protocol operates on an entire log array, rather than on
> single log entries. Raft may superficially resemble Multi-Paxos. However, the
> two protocols are fundamentally different; Multi-Paxos does not consider a log
> array as a cohesive whole. As such, Raft bears a closer resemblance to Classic
> Paxos, functioning as a system that manages a singular value. This **singular
> value** is designed to be extensible—it can grow but is not reducible.

### Implementation of Commit in Raft

Achieving the initial requirement for a commit is straightforward: simply ensure
that the node sets for writing and reading have at least one node in common.
Raft's complexity (and every other consensus) mainly lies in fulfilling the
second requirement:

The guarantee that a specific log array **will be selected** implies a **total
order relationship** between each log array.  This necessitates an attribute for
each log array that reflects its **greatness**.  The log array written by any
new Leader must be greater than all others log array.  To manage this, Raft
employs the concept of a `term` to quantify the greatness of log array,
requiring that terms increase in a global and monotonic fashion.  Since a `term`
may encompass multiple log entries, the greatness of a log array is designated
by the `term` and `index` of the last log entry, known as the `last-log-id`: `(term,
index)`.

Thus, the concept of commit can be divided into two parts:

- Firstly, the reader (Candidate) identifies the log array that possess the
  greatest `last-log-id` and deems them as committed.

- On the other hand, the writer (Leader) considers the data committed only after
  writing log array with the largest `last-log-id`.

The reader's behavior in Raft is such that: during leader election, only the
Candidate with the greatest `last-log-id` can be elected as Leader.

The writer's behavior in Raft is such that, before replicating any logs, a
**Candidate must prevent the commitment of data with smaller `last-log-id`**.
This is crucial because if such data were committed, and the data the Candidate
intends to write is greater (owing to a higher `last-log-id`), then the other
written data would not be chosen by a subsequent Candidate. This leads to a loss
of committed data, violating the principles of commitment. Therefore, during the
election phase, a Candidate must propagate its `term`(`term` identifies the greatness
of the log array it will replicate) to a majority and establish
an agreement with other writers (Leaders) to cease writing upon encountering a
higher `term`.


Therefore, the election mechanism in the Raft protocol involves:
a Candidate acts both as a reader, choosing previously committed data,
and as a writer preparing to replicate log array. This is achieved by broadcasting
its `term` to prevent the replication of log array with a smaller `last-log-id`.


> ```
> RequestVote RPC:
>
> Arguments:
>     term         : candidate’s term
>     candidateId  : candidate   requesting vote
>     lastLogIndex : index of candidate’s last log entry (§5.4)
>     lastLogTerm  : term of candidate’s last log entry (§5.4)
>
> Results:
>     term         : currentTerm, for candidate to update itself
>     voteGranted  : true means candidate received vote
>
> Receiver implementation:
>     1. Reply false if term < currentTerm (§5.1)
>     2. If votedFor is null or candidateId, and candidate’s log is at
>        least as up-to-date as receiver’s log, grant vote (§5.2, §5.4)
> ```


[Vote]: `crate::Vote`
[`Leading`]: `crate::Leading`
[docs-LeaderId]: `crate::docs::tutorial#leaderid`
[etcd-raft-handle-term]: https://github.com/etcd-io/raft/blob/4fcf99f38c20868477e01f5f5c68ef1e4377a8b1/raft.go#L1053
