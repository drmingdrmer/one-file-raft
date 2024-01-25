## LeaderId

在 `one-file-raft` 中，`LeaderId` 是一个 `u64` 类型的节点ID，
用以代表选举成功后的领导者或选举过程中的候选人。
引入 `LeaderId` 的目的是为节点ID定义一个 `PartialOrd` 排序关系，利用这个关系简化选举时节点的选择逻辑：

忽略 term 等其他因素，如果一个节点已对 `LeaderId_1` 投票，则只有当 `LeaderId_2 >= LeaderId_1` 时，它才能对 `LeaderId_2` 投票。

在标准 Raft 协议中，同一 term 内最多只能给一个候选人投票，这意味着如果不考虑 term，两个不同的 `LeaderId` 无法比较大小。
这就是我们实现 `PartialOrd` 的基础。

将这种逻辑转换为 `PartialOrd` 关系，是为了后续将其他条件如 term 和 RPC 类型也抽象为 `PartialOrd` 的关系并封装在 `Vote` 结构中，
通过 `Vote` 的 `PartialOrd` 关系, 一个简单的大小比较就可以判断所有涉及 Leader 合法性的代码（接受或拒绝来自某领导者的 RPC 请求）。
这样也能将正确性测试集中于 `PartialOrd` 的实现上，而非分散在代码库的不同位置。我们将在后面看到这种简化逻辑的强大作用。

```rust
pub struct LeaderId(pub u64);

impl PartialOrd for LeaderId {
    fn partial_cmp(&self, b: &Self) -> Option<Ordering> {
        [None, Some(Ordering::Equal)][(self.0 == b.0) as usize]
    }
}
```


## Vote

在 one-file-raft 中, 我们将所有关于 `term` 和 `voted_for` 的判断和操作都抽象到一个 `Vote` 的概念中:

在标准 Raft 中, 每次收到一个来自外部的消息, 都要验证其合法性:
- 1) node 收到 elect 请求, 如果 `req.term > self.term`, 则更新自己的 `term`, 并设置 `self.voted_for` 为请求的 LeaderId, 并回复 OK
- 2) node 收到 elect 请求, 如果 `req.term == self.term`, 那么除非自己的 `self.voted_for` 跟请求的 LeaderId 相同, 否则回复 Reject.
- 3) node 收到 AppendEntries(或InstallSnapshot) 请求, 如果 `req.term >= self.term`, 则更新自己的 `term`, 并设置自己的 `voted_for` 为 LeaderId.
- 4) Leader 收到一个请求的 reply 时, 如果发现自己的 `self.term < reply.term`, 则说明自己已经过时了, 需要退位, 并更新自己的 `term` 和 `voted_for`.

> 对标准 Raft 的这些逻辑可以参考 [etcd-raft 中对 `term` 操作的实现][etcd-raft-handle-term];

one-file-raft 中为了减少 ~代码行数~ 思维负担, **对这些逻辑都合并到了 `Vote` 中**:

- 观察其中 1) 2) 4) 中, 可以将更新 `(term, voted_for)` 的条件归结为:

    ```text
    if (term, voted_for) > (self.term, self.voted_for) {
        self.term = term;
        self.voted_for = voted_for;
    }
    ```

    其中 `voted_for` 的类型为上面定义的 [LeaderId][docs-LeaderId], 注意它的 PartialOrd 实现, 使得 2) 成立.

- 但是 3) 是比较特殊的, 在 AppendEntries 请求中, **即使请求的 `term` 跟自己的 `term` 相同**, 也允许将自己的 `voted_for` 更新为请求者的.
  这是因为一个 AppendEntries 请求一定是 Leader 发出的, Leader 一定收到了半数以上的成员的认可, 所以自己本地的 `voted_for` 一定是没被半数成员认可的, 所以本地的 `voted_for` 可以被替换掉.
  即, 当 `term` 一样时, 被半数以上认可的 `voted_for` 可以替换掉没被半数认可的 `voted_for` 的值, 在 one-file-raft 中,我们把被半数以上认可的信息(包括这里的 elect 请求, 也包括日志的复制)称为 `committed`.

因此, 上面所有的更新条件都可以归结为 one-file-raft 中的 [Vote][] 定义:

```rust
#[derive(PartialOrd)]
pub struct Vote {
    pub term: u64,
    pub committed: Option<()>,
    pub voted_for: LeaderId,
}
```

注意到 `Vote` 继承了一个 `PartialOrd` 关系: 顺次比较 `term`, `committed`, `voted_for`;
这样所有对 `term, voted_for` 的操作(`handle_elect`, `handle_elect_reply`, `handle_append_entries`, `handle_append_entries_reply`)都可以统一为一个逻辑:

```text
if vote > self.vote {
    self.vote = vote;
}
```

在 one-file-raft里, 不论是哪个阶段, 对 Leader 合法性的处理只需: **将 `vote` 更新为更大的值**.
这也体现了分布式一致性的目的就是定序的原则, `vote` 的大小顺序定义了一组事件(隶属于某个 Leader 的 log)的先后顺序.
某种程度上也可以将 `Vote` 看做 Raft 中的虚拟时间的概念, 从 **时间** 的视角来考虑, Raft 各个环节的逻辑不再显得零散孤立, 它们都在围绕一个的中心: **如何在单调的时间上记录连续的事件**

[Vote]: `crate::Vote`
[docs-LeaderId]: `crate::docs::tutorial_cn#leaderid`
[etcd-raft-handle-term]: https://github.com/etcd-io/raft/blob/4fcf99f38c20868477e01f5f5c68ef1e4377a8b1/raft.go#L1053
