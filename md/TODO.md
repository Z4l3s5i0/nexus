## benchmarking
use 
- methods benching with [microbench](https://docs.rs/microbench/latest/microbench/)
- programm benching with [hyperfine](https://github.com/sharkdp/hyperfine)
- on linux you have also access to `strace` that is wide-used..

--> to bench simple logging and syscalls per log. difference in amount? difference in speed?
## plotting
use:
- gnuplot
- matplot/seaborn

## bugs
- freeze at high contention workloads when reorging on reorging --> too long execution time
- adjust gaslimit for contender workload spam
- hive bugs:
  - blobs
  - reorg (forkchoiceupdate: context deadline exceeded)
### mitigation ideas
- evm parallelization and sidechain building with swapping
- storage for (pre)validated payloads (one time execution)
- snapshotting for freezes, rollbacks

## metrics
Analyse papers.
at the moment:
- submit/commit ratio
- latency?
- syscalls?
- block and transaction executiontime

## experiments
- run a geth node gather the same metrics for a comparison
- to compare is the 10 node uber submit/commit ratio
- 