# one_file_raft

This is a concise, demonstrative implementation of the Raft consensus algorithm
contained within a single Rust file, approximately 300 lines in length.

The primary objective is to provide an educational demo that shows the core
principles of a distributed consensus protocol, free from the complexities of
application-specific business logic, edge case management, and error handling.

The implementation focuses on the fundamental aspects of Raft, such as leader
election, log replication and log commit, while omitting advanced features like
log compaction and log purging.

```shell
./loc.sh
     288
```

For a production use of Raft, refer to [Openraft](https://github.com/datafuselabs/openraft)
