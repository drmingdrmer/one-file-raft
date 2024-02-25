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

[Vote]: `crate::Vote`
[docs-LeaderId]: `crate::docs::tutorial#leaderid`
[etcd-raft-handle-term]: https://github.com/etcd-io/raft/blob/4fcf99f38c20868477e01f5f5c68ef1e4377a8b1/raft.go#L1053
