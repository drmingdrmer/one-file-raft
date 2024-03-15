## LeaderId

In our demonstration of `one-file-raft`, the concept of `LeaderId`, a `u64` node identifier, emerges as central to understanding leadership within the Raft protocol. This identifier serves two roles: it marks the `Leader` that has won an election, or `Candidate` that is during the election.

`LeaderId` introduces a `PartialOrd` ordering. This ordering aids in decision-making: a node voted for `LeaderId_1` can only vote for `LeaderId_2` if `LeaderId_2 >= LeaderId_1`, excluding other factors like the term.

In the standard Raft protocol, within the same term, a vote can be cast for only one candidate, which means that without considering the term, two different `LeaderIds` cannot be compared. This is the basis for our implementation of `PartialOrd`.

Converting this logic into a `PartialOrd` relationship allows us to later abstract other conditions such as term and RPC types into the `PartialOrd` relationship and encapsulate them within the `Vote` structure. With the `PartialOrd` relationship of `Vote`, a simple comparison can determine the legitimacy of the Leader (accepting or rejecting RPC requests from a certain leader). It also concentrates correctness testing on the implementation of `PartialOrd`, rather than being dispersed throughout the codebase. We will see the powerful effect of this simplified logic later on.

```rust,ignore
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

Therefore, all the above updating conditions can be summarized with the definition of [`Vote`][] in one-file-raft:

```rust,ignore
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
This also reflects the essential of distributed consensus, which is ordering events, where the order of `vote` defines the sequence of events(logs).
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


> ```text
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



## Replication: Time and Event

Raft (or other distributed consensus protocols) can be seen as consisting of two
orthogonal and independent problems:

- 1) Horizontally, it addresses how data is distributed across multiple nodes:
  for example, how read-quorums and write-quorums are agreed upon to ensure
  visibility between reads and writes, as well as membership changes, all of
  which fall under this horizontal aspect.

- 2) Vertically, it primarily resolves the problem of ordering events. Here, two
  concepts are introduced: **monotonic time** and the **monotonically increasing
  history of events** that occur over this **monotonic time**. The design of
  Raft's `Election` and `AppendEntries` mechanisms are aimed at solving these
  issues.

As we know that **there are no consensus issue in a single-threaded environment**,
this is because there are some fundamental assumptions present in a
single-threaded environment that do not exist in a distributed setting.
Raft aims to fill in these missing pieces,
thus providing consensus in a distributed environment that are similar to those
in a single-threaded context.
The assumptions that exist only in a single-threaded environment include:

- The **time** used by the system is monotonically increasing and does not go backward;
- At any moment in **time**, only one **event** occurs;
- New **events** can only take place at the current **time**, not at any past time;
- Once an **event** has occurred, it does not disappear.

These four assumptions are crucial for consensus.
It is clear that they are obviously valid in a single-threaded environment:

- **Time** is monotonically increasing: In a single-threaded environment,
  because a wall clock is used, the monotonicity of time is an obvious
  guarantee;

- Only one **event** at any moment: Also apparent in a single-threaded context,
  as two operations on the same variable are always sequential;

- New **events** can only occur at the current time: In a single-threaded
  environment, each write to a variable necessarily happens at the current
  moment of the wall clock;

- **Events** do not disappear: In a single-threaded environment, all operations
  on a variable are already reflected in its final value;

Because we live within the realm of the wall clock, consensus is an obvious
outcome in a single-threaded environment.
However, in a distributed setting like Raft, the time is virtual, and we live
outside its virtual time.
Raft needs to re-establish these assumptions to achieve consensus.

Now let's review Raft from the perspective of **time** and **events**:

- In the [Vote][docs-Vote] section, we see that `term` (the most important
  attribute in Vote) is a globally monotonically increasing variable, which is
  also monotonically increasing on each node;
  It can be regarded as the concept of **virtual time** in Raft.

- In the [Commit][docs-Commit] section, we observe that the state of the entire
  system, which is represented by **log array**, is also globally
  **monotonically increasing**, and it is **monotonically increasing** on each
  node as well (here we can ignore the `last-log-id` rollback caused by
  truncating logs: because the rollback is always for uncommitted logs);
  The increase here is reflected in the fact that the `last-log-id`, which
  determines the greatness of **log array** in Raft, is monotonically
  increasing.


From these two concepts, we can see that Raft (or any distributed consensus
algorithm) is the same as that of a single-threaded system:
Raft just **substitutes the common-sense wall clock with a well-defined
virtual time, and define event as operation logs**:

- The system's **time** (term) is monotonically increasing and does not rollback;

- At any moment, there is only one **event** (a single Leader for each term) writer;

- New **events** can only occur at the current **time** and not at a past
  **time** (a Leader can only propose logs of its own term, and if an Election
  with a larger term is completed, a Leader with a smaller term is no longer
  allowed to commit data);

- The history of **events** cannot be rolled back (committed logs cannot be
  lost, and candidates choose the log array with the largest `last-log-id`).

Raft ensures these four assumptions in a distributed environment, thereby
providing consensus.


### Implement Replication

Based on the above abstraction, viewing Raft as a monotonic sequence of
time+events, the replication in one-file-raft is shown as:

-   The recipient of replication requests (Follower): only accepts replication
-   requests that keep time and events monotonically increasing;

    The operations of the replication request include:
    - Updating the current time (term) to a greater value;
    - And updating the event history (log array) to a greater value.

-   The initiator of replication (Candidate/Leader): replicates its own time
    (term) and event history (log array) to other nodes;

    And only after successfully updating the system to a new **time** (term) is
    it permitted to write new **events**(log).

    Because in a distributed system, writing an event at a greater time can be
    concurrent with writing an event at a smaller time.
    Therefore updating the time and writing the event must be two separate
    operations:
    - The first step blocks the writing of events at smaller times (which is the
      Election phase);
    - only then can data truly be replicated(which is the AppendEntries phase).

Because there is only one replication logic,
in one-file-raft, there is only one RPC: a single `Replicate` RPC.
When a Follower handles a `Replicate` request,
it checks whether both the vote (term) and last-log-id are **greater than or
equal** its own as a condition for the legitimacy of the request:

```rust,ignore
fn handle_replicate_req(&mut self, req: Request) -> Reply {
    let is_granted = vote > self.sto.vote;
    let is_up_to_date = req.last_log_id >= self.sto.last();

    if is_granted && is_up_to_date {
        // ...
    }
}
```

For the same reasons, one-file-raft does not differentiate between Candidate and
Leader, or RequestVote and AppendEntries.
The initiator of replication(Candidate or Leader) simply broadcasts its local
**time** (term) and **event history** (log array) to other nodes.
If a replication RPC is accepted by a majority, it indicates that no more data
can be committed at time points(smaller term) before the current time point(term),
and the initiator can now add events of the current time (term) to the **event history**(log
array).

### Inference: Initial Commit Optimization

Building on the time and events-based interpretation of Raft, another potential
optimization emerges:
Standard Raft could effectively **transmit logs to other nodes during the
Candidate phase as part of the RequestVote requests**.

If the other nodes deem the `RequestVote.term` to be equal to or greater than
their own, and `RequestVote.last_log_id >= self.sto.last_log_id`, they would
process the incoming logs from the Candidate in the same manner as they would
for AppendEntries requests.

This optimization allows Raft to complete the initial commit without waiting for
the replication of the next blank log,
thereby minimizing system downtime by one round-trip time (RTT) during Leader transitions.
(Note: This optimization has not been implemented in one-file-raft yet.)


## Replication Protocol

Based on the principles outlined above, the implementation of the one-file-raft
Replication protocol includes three parts:
- Sending Replication Request,
- Handling Replication Request,
- Handling Replication Reply.

### 1: Sending Replication Request

In one-file-raft, the initiator of Replication does not differentiate between a
`Candidate` and a `Leader`.
There is only one [`Leading`][] that plays both roles of `Candidate` and `Leader`.
Both `RequestVote` and `AppendEntries` requests are represented by a single
[`Request`][] structure.
All Replication Requests are initiated by the [`send_if_idle()`][] function.

[`send_if_idle()`][] uses [`Progress`][] to track the replication progress of
each target, recording:
- `acked`: The highest log-id confirmed to have been replicated;
- `len`: The maximum log index + 1 on the `Follower`;
- `ready`: Whether it is currently idle (no inflight requests awaiting a response)

```rust,ignore
struct Progress {
    acked: LogId,
    len:   u64,
    ready: Option<()>,
}
```

First, [`send_if_idle()`][] checks the current target node to see if it has
completed the previous replication via [`Progress`][].
If it has, a `Replicate` request is sent; otherwise, it returns immediately.
The `ready` is a container that holds at most one token (token is a `()`).
The token is taken out when a Replication request is made and put back upon
receiving a reply:

```rust,ignore
// let p: Progress
p.ready.take()?;
```

Second, it calculates the starting position of the logs to be sent.

Since in Raft, the `Leader` initially does not know the log positions of each
Follower, a multi-round RPC binary search is used to determine the highest log
position on the `Follower` that matches the `Leader`.

The Leader maintains a range `[acked, len)` inside [`Progress`][], indicating the
binary search range.
Here, `acked` is the highest log-id confirmed to be consistent with the Leader,
and `len` is the log length on the Follower. Initially, this search range is
initialized as `[LogId::default(), <leader_log_len>)`.

Note that `leader_log_len` might be less than the length of logs on the
`Follower`.
However, since logs that exceed the `Leader`'s logs on the `Follower` are
definitely not committed and will eventually be deleted, there's no need to
consider these excess logs when searching for the highest matching log-id matching.

To compute the starting log position `prev`, simply take the midpoint of `[acked, len)`.
After several repetitions, `acked` will align with `len`:

```rust,ignore
// let p: Progress
let prev = (p.acked.index + p.len) / 2;
```

The third step is to assemble a Replication RPC: [`Request`][].

- Validation part:
  As mentioned earlier, it includes the `Leader`'s [`Vote`][] and `last_log_id`.
  Both values must be greater than or equal to the corresponding `Follower`'s for
  the request to be considered valid; otherwise, it will be rejected.

  ```rust,ignore
  let req = Request {
      vote:        self.sto.vote,
      last_log_id: self.sto.last(),
      // ...
  }
  ```

- Log data section:
  This includes a section of logs starting from the previously calculated
  starting point `prev`,

  ```rust,ignore
  let req = Request {
      // ...
      prev: self.sto.get_log_id(prev).unwrap(),
      logs: self.sto.read_logs(prev + 1, n),
      // ...
  }
  ```

- Finally, it includes the `Leader`'s commit position so that the `Follower` can
  update its own commit position in a timely manner:

  ```rust,ignore
  let req = Request {

      // Validation section

      vote:        self.sto.vote,
      last_log_id: self.sto.last(),

      // Log data section

      prev:        self.sto.get_log_id(prev).unwrap(),
      logs:        self.sto.read_logs(prev + 1, n),

      commit:      self.commit,
  };
  ```


### 2: Handle Replication Request

All of the RPC request handling code in one-file-raft is contained within the
following succinct 17 lines ([`handle_replicate_req()`][]):

```rust,ignore
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
```

A Follower processes Replication requests in two main steps:

- First, it validates the request. If the request is invalid, it
  immediately responds with a Reject.
- Next, if the request is valid, the Follower updates its own **time** and
  **event** history, then responds with an OK.

As previously mentioned, the validity of a Replication request from a Leader
hinges on two factors: its `vote` (or term) and `last_log_id` must be at least
equal to those of the Follower.
Put simply, the Leader must be on the same or a more recent **time**, and it
must possess a more up-to-date **event** history.

Standard Raft differentiates the validity checks based on the type of RPC:

- For `RequestVote` requests:
  A node can accept the request only if its current `voted_for` is null,
  indicating it hasn't voted yet.
- For `AppendEntries` requests:
  A node can accept the request even if its `voted_for` is **NOT** null, indicating
  it has already voted.

By contrast, one-file-raft streamlines this process into two straightforward
comparisons.
The overall structure of Replication request handling looks like this
([`handle_replicate_req()`][]):

```rust,ignore
fn handle_replicate_req(&mut self, req: Request) -> Reply {

    let is_granted   = vote > self.sto.vote;
    let is_upto_date = req.last_log_id >= self.sto.last();

    if is_granted && is_upto_date {
        // ... to be continued below ...
    }
}
```


If the Replication request is validated, the Follower updates its local
**time** and **event** history to reflect the values in the request.
- The **time**(`vote`) is updated via a direct assignment,
- while the **event** history is appended with the logs included in the request.

Similar to standard Raft, the Follower appends the new logs only after ensuring
there's continuity with its existing logs: if there's a mismatch with the
Leader's logs, the local logs must be **uncommitted** and are thus truncated, in
anticipation of the Leader's subsequent Replication efforts.

```rust,ignore
{
    if is_granted && is_upto_date {
        // ... continued below ...
        if self.sto.get_log_id(req.prev.index) == Some(req.prev) {
            self.sto.append(req.logs);
        } else {
            self.sto.truncate(req.prev);
        };
    }
}
```

Finally, the Follower sends a `struct Reply` to report the outcome of the Replication request:

- `granted` indicates the request's validity, effectively verifying the
  Leader's time (`vote`) and event history (`log`).
- `vote` represents the Follower's current **time**.
- `log` reflects the result of writing the request's logs to the Follower's local storage.
    - `Ok(LogId)` indicates successful acceptance of the log and returns the
      largest known log id aligned with the Leader.
    - `Err(u64)` indicates the logs are not continuous and cannot be accepted,
      returning the Follower's own largest log index + 1, informing the Leader
      that a binary search is only needed before this position.

```rust,ignore
pub struct Reply {
    granted: bool,
    vote:    Vote,
    log:     Result<LogId, u64>,
}
```


[`Vote`]: `crate::Vote`
[`Leading`]: `crate::Leading`
[`Progress`]: `crate::Progress`
[`Request`]: `crate::Request`
[`send_if_idle()`]: `crate::Raft::send_if_idle`
[`handle_replicate_req()`]: `crate::Raft::handle_replicate_req`

[docs-LeaderId]: `crate::docs::tutorial#leaderid`
[docs-Vote]: `crate::docs::tutorial#vote`
[docs-Commit]: `crate::docs::tutorial#commit`
[etcd-raft-handle-term]: https://github.com/etcd-io/raft/blob/4fcf99f38c20868477e01f5f5c68ef1e4377a8b1/raft.go#L1053
