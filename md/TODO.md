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
- high contention workloads do not include all the transactions,.. reorg back to few transactions?

## metrics
Analyse papers.
at the moment:
- submit/commit ratio
- block and transaction executiontime
- latency?
- syscalls?

## experiments
- run a geth node gather the same metrics for a comparison
- to compare is the 10 node uber submit/commit ratio
- 