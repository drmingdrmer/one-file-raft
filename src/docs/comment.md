## LeaderId

In our demonstration of `one_file_raft`, the concept of `LeaderId`, a `u64` node identifier, emerges as central to understanding leadership within the Raft protocol. This identifier serves two roles: it marks the `Leader` that has won an election, or `Candidate` that is during the election.

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

