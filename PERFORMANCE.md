# Performance: ghostpp-rs vs. original GHost++

Long-run production comparison of **ghostpp-rs** against the original C++
[GHost++](https://github.com/uakfdotb/ghostpp): both bots on the same ~1 GB
Vultr VPS (Debian 13), connected to the same PVPGN server and autohosting the
same map, sampled with `top` after a long stretch of continuous operation.

```
    PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND
 248352 root      20   0  445040 123656   8244 S   0.3  12.6  19:00.71 ghostpp     ← ghostpp-rs (v0.1.1)
 251427 root      20   0  467052 283212   6744 S   0.3  28.8  15:17.41 ghost++     ← original C++
```

| | GHost++ (C++) | ghostpp-rs (v0.1.1) |
|---|---|---|
| RSS | 276.6 MB | **120.8 MB** |
| CPU % | 0.3 | 0.3 |
| Share of the 1 GB VPS | 28.8 % | 12.6 % |

Less than half the memory at identical CPU usage. The snapshot was taken while
hosting: since v0.1.1 the raw map file (~90 MB here) is loaded only while games
exist, so an idle ghostpp-rs drops further, to around 20 MB.
