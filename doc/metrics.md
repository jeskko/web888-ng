# Metrics

`GET /metrics` serves the server's state in Prometheus' text format when
`[web] metrics = true`, to the clients `[web] metrics_allow` lists
(addresses or networks; empty, the default: nobody, 403). Both apply
live, without a restart. There's no login. Behind a proxy listed in
`trusted_proxies` the forwarded client is the one checked.

```toml
[web]
metrics = true
metrics_allow = ["192.168.1.0/24", "127.0.0.1"]
```
 Each scrape reads the counters and gauges as
they are then; the history is the scraper's. A 15 s scrape interval is
plenty. Every name starts with `web888d_`. The code is
`crates/web/src/metrics.rs`.

```yaml
# prometheus.yml
scrape_configs:
  - job_name: web888d
    scrape_interval: 15s
    static_configs:
      - targets: ["web888.local:8073"]
```

A Grafana dashboard for these: [grafana/web888d.json](grafana/web888d.json)
(Dashboards → New → Import; pick the Prometheus data source). Rows:
overview, listeners and channels, the DSP pool (load, time and jobs by
kind, shedding, lost blocks), waterfalls, skimmers (spots, CPU and slots
not decoded in full per band, a table of the bands now), uploads and log
sinks, the system. Restarts are marked.

## What there is

| Metric | Type | Labels | What |
|---|---|---|---|
| `build_info` | gauge (1) | `version`, `backend`, `variant` (hf, vhf), `rx` (ddc, bins), `signature` | the build and the radio |
| `start_time_seconds` | gauge | | when the server started (Unix time) |
| `adc_hz` | gauge | | the ADC's rate |
| `sessions` | gauge | `protocol` (web, kiwi) | listener sessions |
| `channels` | gauge | | RX channels |
| `channels_in_use` | gauge | `class` (web, protocol, skimmer, admin) | channels leased (Kiwi listeners are web, HPSDR receivers protocol) |
| `lease_queue` | gauge | `class` | lease requests waiting |
| `audio_frames_dropped_total`, `wf_frames_dropped_total` | counter | `protocol` | frames dropped: the client didn't keep up |
| `adc_overloads_total` | counter | | blocks with an ADC overload |
| `rx_frames_total` | counter | | RX frames from the hardware |
| `clock_ppm` | gauge | | the ADC clock's error from the GPS PPS (0 without one) |
| `hw_rate_ok` | gauge | | the hardware keeps up with the RX rate |
| `pool_busy_seconds_total` | counter | `worker` | DSP worker busy time |
| `pool_jobs` | gauge | `worker`, `kind` | jobs on a worker: `audio` (a listener's), `waterfall` (a view from the WF FFT), `bins` (a view from the channelizer's bins), `skimmer` (a band's audio), `other` |
| `pool_job_seconds_total` | counter | `worker`, `kind` | worker time by kind of job, estimated from every 8th round |
| `pool_shed_level` | gauge | `worker` | load shedding level (0: none) |
| `pool_backlog_blocks` | gauge | `worker` | most RX blocks waiting in the last second |
| `pool_rx_dropped_total`, `pool_wf_dropped_total` | counter | `worker` | blocks a worker lost |
| `wf_views_open` | gauge | | waterfall views |
| `wf_views`, `wf_view_sessions`, `wf_view_fps` | gauge | `decim` | views, sessions watching them and lines per second, by span (the ADC rate over the span's) |
| `skim_slots_total`, `skim_spots_total` | counter | `mode`, `dial` (Hz), `band` | slots decoded and spots per skimmer band |
| `skim_slots_dropped_total`, `skim_slots_cut_total`, `skim_slots_shed_total` | counter | same | slots dropped (decoders behind), cut short by the deadline, not decoded (band shed) |
| `skim_shed`, `skim_granted` | gauge | same | the band is shed now; it has an RX channel |
| `skim_cpu_seconds_total` | counter | same | decoder CPU time |
| `pskreporter_enabled`, `_pending`, `_last_sent_seconds` | gauge | | PSKReporter uploading |
| `pskreporter_reports_total`, `_datagrams_total` | counter | | spots and datagrams sent |
| `pskreporter_skipped_total` | counter | `reason` (not_live, recent) | spots not sent |
| `wsprnet_enabled`, `_pending`, `_error`, `_last_sent_seconds` | gauge | | WSPRnet uploading; the last upload failed |
| `wsprnet_spots_total`, `_failed_total`, `_skipped_total` | counter | | spots sent, given up on, not sent (not live) |
| `gpsd_connected`, `gpsd_fix_mode`, `gpsd_last_fix_seconds` | gauge | | gpsd (when set) |
| `log_sink_configured`, `log_sink_connected` | gauge | `sink` (mqtt, syslog) | a server is set; connected |
| `log_sink_messages_total`, `_dropped_total`, `_errors_total` | counter | `sink` | messages sent and dropped; failed connects and sends |
| `system_cpu_seconds_total` | counter | `mode` (busy, idle) | all CPUs |
| `temperature_celsius` | gauge | | the SoC's |
| `load_average` | gauge | `period` (1m, 5m, 15m) | |
| `memory_bytes` | gauge | `kind` (total, available) | the system's memory |
| `process_resident_bytes` | gauge | | the server's |
| `filesystem_size_bytes`, `filesystem_avail_bytes` | gauge | `path` (/, /tmp, /data) | (/tmp is in RAM on the Web-888) |
| `thread_cpu_seconds_total` | counter | `thread` | CPU time per thread name (`dsp0`, `skim0`, `hpsdr`, ...) |

Not here: who is connected (addresses, frequencies). That's the admin
API's (`/api/admin/sessions`, `/api/admin/leases`).

## Queries

```promql
# CPU busy, %
100 * rate(web888d_system_cpu_seconds_total{mode="busy"}[5m])
  / ignoring(mode) sum without(mode) (rate(web888d_system_cpu_seconds_total[5m]))

# each DSP worker's load (1 = a core), and what it goes to
rate(web888d_pool_busy_seconds_total[1m])
sum by (worker, kind) (rate(web888d_pool_job_seconds_total[1m]))

# audio lost: blocks a worker dropped, frames dropped to clients
rate(web888d_pool_rx_dropped_total[5m])
rate(web888d_audio_frames_dropped_total[5m])

# skimmers: slots not decoded in full, per band, as a share of slots
(rate(web888d_skim_slots_cut_total[1h]) + rate(web888d_skim_slots_shed_total[1h]))
  / rate(web888d_skim_slots_total[1h])

# spots per hour by band and mode
3600 * sum by (band, mode) (rate(web888d_skim_spots_total[1h]))

# listeners by protocol, HPSDR receivers
web888d_sessions
web888d_channels_in_use{class="protocol"}

# uptime
time() - web888d_start_time_seconds
```
