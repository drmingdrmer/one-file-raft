## LeaderId

在 `one_file_raft` 中，`LeaderId` 是一个 `u64` 类型的节点ID，
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

