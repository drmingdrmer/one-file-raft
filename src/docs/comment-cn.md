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

```rust,ignore
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

因此, 上面所有的更新条件都可以归结为 one-file-raft 中的 [`Vote`][] 定义:

```rust,ignore
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


## Commit

在任何一个 distributed consensus 中, `commit` 都是一个核心概念.
它也是容易被忽略的一个概念, 大部分时候它并没有被严格的去描述,
因为在单线程系统中, commit 是一个 **trivial** 的概念: 写入一个变量后, 它一定能被后续的读取者看到.
但在分布式系统中, commit 的概念发生了变化, 写入不一定能读到, commit 的概念需要重新定义,
分布式系统区别于单机系统的独有问题可以认为都源于 commit 概念的变化.

一个值, 通过一系列步骤写入, 一定能通过一系列步骤被读出, 则它就是 committed.
这表示 commit 是一个 write 和 read 双方必须共同遵守的一个约定.

> 例如, 在一个不考虑宕机的5节点系统里,
> 如果只考虑一次写入和读取, 那么对 write 和 read 的 commit 的约定可以是以下任意一个:
> - write 阶段是写全部5个节点, read阶段任意读1个节点;
> - write 阶段是任意写4个节点, read阶段任意读2个节点;
> - write 阶段是任意写3个节点, read阶段任意读3个节点;
> - write 阶段是任意写2个节点, read阶段任意读4个节点;
> - write 阶段是任意写1个节点, read阶段读所有5个节点.

### Raft 中 Commit 的定义

在 Raft 中, commit 的概念是针对 **一组log** 定义的. 一组 log 的 commit 的 write read 协议要求:
- 一组 log 被写入 majority(半数以上节点), 以此保证一定能被后续的读取者(Candidate) 通过访问一个 majority 看到;
- 这组 log 如果被看到, 则一定会被 Candidate 选中作为当前系统的状态变化的日志, 而不选其他也被看见的log.

> Raft consensus的单位是整个一组 log 而不是单条 log. 这是一个常见的误区,
> Raft 和 Multi Paxos 虽然形似但却是完全不同的2个协议, Multi Paxos 没有将一组日志作为一个整体来对待.
> 所以 Raft 跟 Classic Paxos 更相似, 应该认为还是一个单值的系统, 但这个 **单值** 是可以增长的(不能缩短).

### Raft 中 Commit 的实现

Commit 的第一条要求很容易达成, 只需遵循 write 和 read 的节点集合必有交集就可以.
Raft 的大部分逻辑在如何满足第二个需要:

这里提到的某一组 log **一定能被选择** 说明, 每组 log 之间存在一个 **全序关系**,
所以每组 log 需要有一个属性来标识它的 **大小**. 而每个新的 Leader 要写入的新的一组log, 都必须最大,
所以 Raft 中引入一个 term 的概念来标识一组 log 的大小, 且 term 必须全局单调递增.
以及因为每个 term 中允许写入多条log, 所以这个表示每组 log 大小的属性就是: 最后一条日志的 term 和 index, last-log-id: `(term, index)`.

这样, commit 的概念就可以被分成了2个部分:
一方面, reader(Candidate) 看到哪组日志的 last-log-id 最大, 就选择哪组日志作为已 committed 的日志;
另一方面, writer(Leader) 写入了有最大 last-log-id 的日志, 才认为数据已经 committed.

reader 的行为体现在 leader election 时, 持有最大的 last-log-id 的 Candidate 才能被选中作为 Leader;

writer 的行为在 Raft 中的体现是, 在复制任何log之前,
**Candidate 必须阻止其他较小 last-log-id 的数据被 commit**,
因为如果这样的数据被提交, 而自己要写入的数据又比它大(自己有较大的 last-log-id),
那么其他的写入的数据就不会被下一个 Candidate 选中, 导致 committed 数据丢失, 违反了 commit 的原则.
所以 elect 阶段 Candidate 要将 term, 复制到一个 majority,
并以此跟其他 writer (Leader) 约定, 遇到更大的 term 就放弃写入,
因为更大的 term 意味着较小 term 的 Leader 复制的 log, 可能不具有最大last-log-id, 无法达到一定被后续 Candidate 选中的要求.

于是得出了 Raft 协议的选举过程: 当 Raft 选主时,
Candidate 同时作为一个 reader, 读以前已经 committed 的数据;
同时也为后面作为 writer 复制log 做准备, 即通过广播 term 防止较小的 last-log-id 被复制.

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

Raft (或其他分布式一致性协议), 可以看作是由2个正交的, 相互独立的问题构成:

- 1) 在水平维度上, 解决数据如何在多节点间分布: 例如 read-quorum 和 write-quorum 如何约定,
     使读写之间可见, 以及成员变更, 都属于这个水平维度的问题.

- 2) 在纵向维度上, 主要解决定序问题, 这里就引入了两个概念: **单调的时间**
     和在这个 **单调的时间** 上发生的 **单调递增的事件历史**.
     Raft 中的 Elect 和 AppendEntries 的设计就是在解决这些问题.


**单线程环境中没有一致性问题**,
这是因为在单线程环境中存在一些分布式环境中没有的基本假设.
Raft 就是把这些缺少的东西补全,
从而在分布式环境中提供跟单线程类似的一致性特性.
这些只在单线程环境中存在的基本假设包括:

- (系统使用的) **时间** 单调递增, 不会回退;
- **时间** 上任一时刻, 只有一个 **事件** 发生;
- 新 **事件** 只能发生在当前 **时间**, 不能发生在过去的时间;
- 已发生的 **事件** 不会消失;

这4条假设是保证一致性的关键条件, 缺少任一个一致性都无法保证.
显然它们在单线程环境中是成立的:

- **时间** 单调递增: 单线程环境中, 因为使用墙上时钟, 时间单调是一个显然的保证;
- 任一时刻只有一个 **事件**: 单线程环境中, 也是显然的, 因为对同一变量的2个操作总是有先后;
- 新 **事件** 只能发生在当前时间: 单线程环境中, 每次写一个变量必然发生在墙上时钟的当前时刻;
- **事件** 不会消失: 单线程环境中, 对一个变量的所有的操作日志都已经展现为它最终的值了;

因为我们生活在墙上时钟之内, 所以一致性在单线程环境是一个显然的结果,
而在 Raft 这种分布式环境中, 它的时间是虚拟的, 我们生活在它的虚拟时间之外,
Raft 需要重新建立这些假设并达成一致性的目标.

现在从 **时间** 和 **事件** 的角度重新审视 Raft:

- 在 [Vote][docs-Vote] 章节中, 我们看到 `term`(Vote 中最主要的属性) 是一个全局单调递增的变量, 在每个节点上也是单调递增的;
  它可以看做 Raft 中的 **虚拟时间** 的概念.

- 在 [Commit][docs-Commit] 章节中, 我们看到整个系统的状态, 也就是表示系统状态的 **一组log**, 也是全局 **单调递增** 的,
  同时也是在每个节点上 **单调递增** 的(这里可以忽略 truncate 日志时带来的 `last-log-id` 回退: 因为回退的一定是未提交的 log);
  这里的递增表现在 Raft 中决定 **一组log** 的大小的 `last-log-id` 是单调递增的.
  这组log 就是系统发生的所有的 **事件** 的记录, 所以说 **事件** 也是单调递增的.

从这2个概念来看, Raft(或任何一个分布式一致性算法)在定序方面的行为, 与单线程系统就是一样的:
Raft 只是把以前单线程系统中那些 **无需证明** 的事情讲清楚了,
即 **用一个明确定义的虚拟时间替代常识中的墙上时钟, 再用操作日志来描述对变量的操作**:

- 系统里的 **时间**(term) 单调递增, 不允许回退;

- 任一时刻只有一个 **事件**(一个term只有一个Leader) 的写入者;

- 新的 **事件** 只能发生在当前 **时间**, 不能发生在过去的 **时间** (Leader 只能 propose 自己 term 的 log,
  如果更大的 term 的 Election 完成了, 就不允许较小 term 的 Leader 继续提交数据了);

- **事件** 的历史记录不能回退(committed 的 log 不能丢失, Candidate 选择最大
    last-log-id 的那组log);


Raft 在分布式环境保证了这4个假设, 所以在分布式环境中就提供了一致性.


### Replication 的实现

根据以上的抽象, 把 Raft 看做是单调的 时间+事件 变化,
我们就得到了 one-file-raft 中实现复制的协议:

-   复制请求的接受者(Follower): 只允许 时间+事件 都保持单调增的复制请求;

    复制请求的操作包括:
    - 更新当前时间(term) 到更大的值;
    - 以及更新事件历史(log) 到更大的值.

-   复制的发起者(Candidate/Leader): 将自己的时间(term)和历史事件(log)复制给其他节点;

    且只有将系统成功更新到一个新的 **时间**(term) 后, 才允许写入新的 **事件**.

    这是因为分布式中对较大时间的一个事件写入和在较小时间的一个事件写入可以是并发的,
    所以更新时间和写入事件必须是2步操作, 第一步屏蔽掉较小时间的事件写入, 也就是 Election 阶段;
    然后才能真正复制数据, 也就是 AppendEntries 阶段.


因为复制的逻辑只有一个,
所以在 one-file-raft 中, 只需一个 `Replicate` RPC, Follower 处理 `Replicate` 请求时,
检查 vote(term) 和 last-log-id 是否都 **不小于自己的**, 以作为请求合法的条件:

```rust,ignore
fn handle_replicate_req(&mut self, req: Request) -> Reply {
    let is_granted = vote > self.sto.vote;
    let is_upto_date = req.last_log_id >= self.sto.last();

    if is_granted && is_upto_date {
        // ...
    }
}
```

基于同样的原因, one-file-raft 里也不区分 Candidate 和 Leader, 或 RequestVote 和 AppendEntries,
复制的发起者仅仅是将自己本地的 **时间**(term) 和 **事件历史**(log) 广播给其他节点,
如果完成一次复制, 说明自己当前时刻以前的时间点不再能提交任何数据, 自己可以在**事件**历史上添加当前时刻(term)的事件了.


### 推论: 优化初次 Commit

根据 时间+事件 对 Raft 的诠释, 这里我们还可以得到的另一个优化结论:
标准的 Raft 其实也可以 **在 Candidate 阶段, 在 RequestVote 请求中复制 log 给其他节点**;

其他节点如果认为 RequestVote.term 比自己的大, 且 `RequestVote.last_log_id >= self.sto.last_log_id`,
那么就可以像处理 AppendEntries 请求一样把接受 Candidate 传来的日志.
这个优化可以让 Raft 无需等待下一个 blank log 的复制完成就可以完成初次 commit,
在 Leader 切换时减少一次 RPC 的系统 downtime.
(这个优化在 one-file-raft 里还没有实现)


## Replication Protocol

基于以上原理, one-file-raft 的 Replication 协议的实现如下,
包括三部分:
- Sending Replication Request,
- Handling Replication Request,
- Handling Replication Reply.

### 1: Sending Replication Request

因为 one-file-raft 中 Replication 的发起者不区分 Candidate 和 Leader,
只有一个 [`Leading`][] 结构, RequestVote 和 AppendEntries 请求也只由一个
[`Request`][] 负责.  所有的 Replication Request 都是由 [`send_if_idle()`][] 函数发起的.

[`send_if_idle()`][] 用一个 [`Progress`][] 结构追踪每个 Replication target 的进度状态,
它记录了:
- `acked`: 已确认完成复制的最大的 log-id;
- `len`: Follower 本地最大 log index + 1;
- `ready`: 现在是否空闲(没有已发出但没收到应答的请求)

```rust,ignore
struct Progress {
    acked: LogId,
    len:   u64,
    ready: Option<()>,
}
```

第一步, [`send_if_idle()`][] 先通过 [`Progress`][]
检查当前要发送的目标节点是否已完成了上一次的复制,
如果是则发出一个 `Replicate` 请求, 否则直接返回.
这里的 `ready` 是一个存储至多一个 token(`()`) 的容器, 每次出 Replication 请求时把这个 token 拿走, 应答收到后再将它放回去:

```rust,ignore
// let p: Progress
p.ready.take()?;
```

第二步, 计算发出的日志的开始位置.

因为在 Raft 中, 最初 Leader 不知道每个 Follower 的 log 位置,
所以这里用一个多轮RPC 的 binary search 来确定 Follower 上跟 Leader 匹配的最大 log 的位置.

Leader 在 [`Progress`][] 里维护一个范围 `[acked, len)`, 表示 binary search 的查找范围:
其中 `acked` 是对应 Follower 已经确认的, 和 Leader 一致的最大 log-id,
`len` 是 Follower 上的日志长度, 最开始这个查找范围被初始化为: `[LogId::default(), <leader_log_len>)`.

注意这里 `leader_log_len` 有可能是小于 Follower 的 log 的长度的,
但因为当一个 Leader 选出后, Follower 上多出的 log, 一定是没有 committed, 最终是一定会被删掉的,
所以 Follower 上跟 Leader 匹配的最大 log-id 一定不在这个超出的范围, 不需要考虑这部分多出来的 log.

计算发送 log 的开始位置 `prev`: 直接取 `[acked, len)` 的中点, 重复几次后 acked 就跟 len 对齐了:

```rust,ignore
// let p: Progress
let prev = (p.acked.index + p.len) / 2;
```

第三步是组装一个 Replication 的 RPC: [`Request`][].

- 验证部分:
  如前面所述, 它包括 Leader 的 [`Vote`][] 和 `last_log_id`,
  这2个值都要大于等于对应 Follower 的, 才认为是合法请求, 否则会被拒绝.

  ```rust,ignore
  let req = Request {
      vote:        self.sto.vote,
      last_log_id: self.sto.last(),
      // ...
  }
  ```

- log部分:
  它包括从上面计算的起始点位置 `prev` 开始的一段 log,

  ```rust,ignore
  let req = Request {
      // ...
      prev: self.sto.get_log_id(prev).unwrap(),
      logs: self.sto.read_logs(prev + 1, n),
      // ...
  }
  ```

- 最后带上 Leader 的 commit 位置, 以便 Follower 可以及时的更新自己的 commit 位置:

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

one-file-raft 中所有处理 RPC 请求的代码仅以下 21 行 ([`handle_replicate_req()`][]):

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

fn check_vote(&mut self, vote: Vote) -> (bool, Vote) {

    if vote > self.sto.vote {
        self.sto.vote = vote;
    }

    (vote == self.sto.vote, self.sto.vote)
}
```

Follower 处理 Replication 请求的逻辑包括2步:

- 首先验证请求的合法性, 如果不合法直接返回 Reject;
- 然后更新自己的时间和事件历史, 并回复 OK.

上面我们提到, 一个来自 Leader 的 Replication 请求的合法性取决于:
它的 `vote`(term) 和 `last_log_id` 都要大于等于 Follower 的对应值.
或者更直观的理解为, Leader 具有较大的 **时间** 且有较新的 **事件** 历史.

标准 Raft 中请求合法性检查要区分 RPC 类型:

- 对 `RequestVote` 请求:
  节点自己的当前 `voted_for` 为空, 才能接受请求;
- 对 `AppendEntries` 请求:
  节点自己的当前 `voted_for` 不为空, 也可以接受请求;

one-file-raft 相比之, 验证简化成了2个简单的比较,
最终 Replication 请求的处理整体结构如下([`handle_replicate_req()`][]):

```rust,ignore
fn handle_replicate_req(&mut self, req: Request) -> Reply {

    let is_granted   = vote > self.sto.vote;
    let is_upto_date = req.last_log_id >= self.sto.last();

    if is_granted && is_upto_date {
        // ... to be continued below ...
    }
}
```

如果 Replication 请求验证通过, 则把节点本地的 **时间** 和 **事件** 历史更新为请求中的值,
其中时间更新是直接赋值, 事件历史更新是追加请求中的日志.
跟标准的 Raft 一样, 确认请求中的日志跟本地是连续的之后, 才能追加:
如果跟 Leader 发来的日志不一致, 则说明本地日志一定是 **未提交的**, 需要删除,
等待 Leader 下次 Replication.

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

最后, Follower 回复 Replication 请求处理的结果 `struct Reply`:

- 其中 `granted` 表示请求是否被认为是合法的, 也就是验证 Leader
  的时间(`vote`)和事件历史(`log`);
- `vote` 表示 Follower 自己的时间;
- `log` 表示 Follower 将请求中的 log 写到本地后的结果.
    - 其中`Ok(LogId)` 表示成功接受了log, 并返回了已知的跟 Leader 对齐的最大log
      id.
    - 而 `Err(u64)` 表示日志不连续无法接受, 并返回了 Follower 自己的最大log
      index+1, 告知 Leader 只需要在这个位置之前进行二分查找.

```rust,ignore
pub struct Reply {
    granted: bool,
    vote:    Vote,
    log:     Result<LogId, u64>,
}
```


### 3: Handle Replication Reply

one-file-raft 中所有处理 RPC **应答**的代码在 ([`handle_replicate_reply()`][]) 函数内,
它处理几件事情:

- 1. 首先检查当前节点是否还是 Leader, 如果是直接返回;
- 2. 检查请求是否被对端节点接受(granted), 即对端没有接受到其他更大 Leader 的请求, 当前 Leader 还是有效的;
- 3. 更新 Leader 的选举状态;
- 4. 更新到对端 Follower 节点的 log 复制进度;
- 5. 更新 Leader 自己的 commit 位置.


#### 3.1: 检查当前节点是否还是 Leader

因为 one-file-raft 是异步的事件循环的模型, 有可能在发送请求后, Leader 已经被其他节点选中:
`let l = self.leading.as_mut()?` 这一行, 如果当前节点是还是 `leading` 状态,
则返回一个 `leading` 状态的引用, 否则直接返回 `None`.


#### 3.2: 检查请求是否被接受(granted)

前面提到, 对端节点接受一个 replication 请求的条件是: replication 发起者的**时间**(`vote`)和**事件历史**(`last-log-id`)都要比对端节点的大.
因为对端节点返回它处理 replication 请求之后的 `vote`, 处理的方式是把本地的 `vote` 更新成较大值, 所以:
- 只要对端返回的 `vote` 和自己的一样, 就说明对端接受了请求.
- 如果对端返回的 `vote` 比自己的大, 则说明对端已经接受了一个更大的 Leader, 自己的 Leader 已经过时, 需要退位.
- 如果对端返回的 `vote` 比自己的小, 则说明这是一个延迟到达的 replication  应答, 它由一个更早的Leader发出, 可以直接忽略;

检查 replication 应答有效性的代码如下:

```rust,ignore
// let v = self.sto.vote;

reply.vote.term == v.term
    && reply.vote.voted_for == v.voted_for;
```

如果返回的 `vote` 跟自己不一样(只能是更大),
 则更新自己的 `vote`: `self.check_vote(reply.vote);`, 来实现 Leader 的退位: [`check_vote()`][].


#### 3.3: 更新 Leader 的选举状态

在 3.2 步骤通过后, 下一步是更新 Leader 的选举状态,
也就是说是否集群中有一个 quorum 确认了这个 Leader 的**时间**(`vote`),
quorum 确认 Leader 的 `vote` 后, 就可以保证其他在这个**时间**之前的 Leader 都不能复制 log 到一个 quorum,
即无法完成commit,
那么当前 Leader 就可以安全的 propose log 了.

> 如果还有更小的 Leader 可以复制并 commit log, 那么这个 commit 的 log
> 可能不会被下一个 Leader 选择,
> 因为当前 Leader 有更大的 log-id(有更大 term 的log),
> 导致 committed log 丢失.

这时 Leader 把自己的**时间**(vote) 更新为 committed 的状态:
```rust,ignore
self.sto.vote.committed = Some(());
```

并直接 propose 一条空的 log, 这条空日志的目的是让**事件历史**更新到当前**时间**,
因为 Raft 以 last-log-id 来表示**事件历史**的大小:
最大**事件历史**会被下一个Leader选中, 从而保证了**事件历史**的连续性,
也就是保证了**事件**不会丢失, 也就是保证了 commit 的条件.

```rust,ignore
// let l = self.leading.as_mut()?;

l.granted_by.insert(target);

if is_quorum(self.sto.config(), &l.granted_by) {
    self.sto.vote.committed = Some(());

    let (tx, _rx) = oneshot::channel();
    self.net.send(self.id, self.id, Event::Write(tx, Log::default()));
}
```


#### 3.4: 更新 log 复制进度

replication 包括复制 Leader 的**时间**(`vote`)和Leader的**事件历史**(`logs`).
上一步完成了**时间**(`vote`)的确认, 这一步完成 `log` 复制状态的更新.

对端 Follower 节点可能返回2种日志复制状态:
- `reply.log == Ok(LogId)`: 表示对端日志跟 replication 请求中的日志是连续的,
  已经接受了 Leader 的日志, 并返回了已知的跟 Leader 对齐的最大log id;
- `reply.log == Err(u64)`: 表示日志不连续无法接受,
  并返回了对端 Follower 节点上已知的最大的, 跟 Leader 不一致的 log 的 index,
  告知 Leader 只需要在这个位置之前进行二分查找.

```rust,ignore
// let l = self.leading.as_mut()?;

let p = l.progresses.get_mut(&target).unwrap();

*p = match reply.log {
    Ok(acked) => Progress::new(acked, max(p.len, acked.index + 1), Some(())),
    Err(len) => Progress::new(p.acked, min(p.len, len), Some(())),
};
```

Leader 上为每个 Follower 维护一个进度[`Progress`][], 用来追踪每个 Follower 的复制进度.
其中 `acked` 是最大的, 已经确认完成复制的 log-id, `len` 是远端 Follower 最小的跟 Leader 不一致的 log index:

```rust,ignore
pub struct Progress {
    acked: LogId,
    len: u64,
    //...
}
```

因此远端 Follower 上跟 Leader 一致的 log index 就在 `[acked.index, len)` 之间.
每个 replication 请求都从这个范围的中点位置 `m` 作为发送的 log 的起始位置,
并收到一个`Ok` 或 `Err` 应答, 然后将范围更新为`[acked.index, m)` 或 `[m, len)`.
直到 `acked.index + 1 == len`.
后续的仍然选择 `[acked.index, m)` 的中点作为起始, 就相当于直接从最后确认位置 `acked` 继续复制了.


#### 3.5: 更新 Leader 的 commit 位置

完成 log 复制状态的更新后, 最后一步是更新 Leader 的 commit 位置:
即:
1. 如果范围 `[0, i]` 内的 log 都已经复制到了一个 quorum 中, 那么就可以保证下一个 Leader 一定能看到这些log;
2. 并且如果 `i` 位置的 log 肯定能被下一个 Leader 选择,
   即 `logs[i].term` 一定是全局最大的.
   即它只能是当前 Leader 的 `term`: `logs[i].term == self.sto.vote.term`.
   在实现中, 我们是通过判断 `i` 是否比当前 Leader 的第一条`noop_log` 的 index 更大来判断的: `acked.index >= l.log_index_range.0`.

如果满足上面这2个条件, 则说明 `[0,i]` 范围内的 log 已经不会丢失了, 即 committed.

代码中我们寻找到最大的这样的 `i` 来更新 commit index:
- 将所有 Follower 的最大的完成复制的 log id 按照降序排序;
- 对排好序的每个 log id `acked`, 统计有几个 Follower 也至少复制到了这个位置;
- 最后检查 `acked` 是否具有最大 `term`.

```rust,ignore
// Sort all acknowledged log id in desc order
let acked_desc = l.progresses.values().map(|p| p.acked).sorted().rev();

let mut max_committed = acked_desc.filter(|acked| {

    // For each acknoledged log id `acked`,
    // count the number of nodes that have acked log id >= `acked`.
    let greater_equal = l.progresses.iter().filter(|(_id, p)| p.acked >= *acked);

    // Whether `acked` has the greatest `term`
    acked.index >= noop_index
    &&
    // And whether the number of nodes that have acked log id >= `acked` is a quorum,
    is_quorum(self.sto.config(), greater_equal.map(|(id, _)| id))
});
```


[`Vote`]: `crate::Vote`
[`Leading`]: `crate::Leading`
[`Progress`]: `crate::Progress`
[`Request`]: `crate::Request`
[`Progress`]: `crate::Progress`
[`send_if_idle()`]: `crate::Raft::send_if_idle`
[`handle_replicate_req()`]: `crate::Raft::handle_replicate_req`
[`handle_replicate_reply()`]: `crate::Raft::handle_replicate_reply`
[`check_vote()`]: `crate::Raft::check_vote`

[docs-LeaderId]: `crate::docs::tutorial_cn#leaderid`
[docs-Vote]: `crate::docs::tutorial_cn#vote`
[docs-Commit]: `crate::docs::tutorial_cn#commit`
[etcd-raft-handle-term]: https://github.com/etcd-io/raft/blob/4fcf99f38c20868477e01f5f5c68ef1e4377a8b1/raft.go#L1053
