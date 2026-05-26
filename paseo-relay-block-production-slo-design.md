# Paseo Relay Block Production Monitoring

Scope: relay block production for Paseo SLO #2. Finality is out of scope for
this phase.

Target SLO: 99.5% monthly relay availability. On a 30 day window, that leaves
about 3 hours and 39 minutes of error budget.

## Decisions

- Build a small greenfield monitoring stack.
- Use public relay RPC endpoints first.
- Route alerts to Matrix only.
- Add our own relay node scrape in a later phase.
- Use the runtime's expected block time when deriving thresholds.

## Vantage Points

The exporter watches the public relay endpoints from `endpoints.json`.

| Provider | Endpoint |
| --- | --- |
| dwellir | `wss://paseo-rpc.n.dwellir.com` |
| dotters | `wss://paseo.dotters.network` |
| ibp | `wss://paseo.ibp.network` |
| amforc | `wss://paseo.rpc.amforc.com` |

`dotters` and `ibp` may share infrastructure. Treat correlated failures there
with care until we add our own relay node as an independent vantage point.

## Primary Signal

Track how long each endpoint has gone without a new best block:

```text
block_drift_seconds = now - local arrival time of the current best block
```

The exporter subscribes to `chain_subscribeNewHeads` for each endpoint and
stamps the local arrival time of every head. Healthy drift should stay close to
one or two expected block times. During a stall, drift increases once per wall
clock second.

This avoids height polling noise. It also works across runtime upgrades because
the hot path only needs RPC headers.

On startup, seed drift with `chain_getHeader` and the block timestamp inherent
until the first subscribed head arrives.

## Thresholds

Do not hardcode 6 seconds. Read `api.consts.babe.expectedBlockTime` on connect
and reread it after a `spec_version` change. Cross-check with
`2 * api.consts.timestamp.minimumPeriod`.

Current Paseo values:

| Level | Formula | Current value |
| --- | --- | --- |
| Degraded | `5 * expected_block_time` | 30s |
| P0 stall | `10 * expected_block_time` | 60s |

BABE can skip slots under normal conditions, so the thresholds are expressed as
skipped-slot tolerance instead of fixed seconds.

## Quorum Rule

Use the freshest healthy vantage point to decide whether the chain is producing:

```promql
paseo_relay:block_drift:min =
  min(paseo_block_drift_seconds and on(provider) (paseo_rpc_up == 1))
```

A single lagging provider should not page. The chain is treated as stalled only
when every healthy vantage point has stale block drift.

Provider-level drift is still recorded for dashboards and provider health work.

## Exporter

`paseo-chain-exporter` bridges public RPC into Prometheus. Native node metrics
already exist on nodes we operate, but public providers do not expose their
`:9615` endpoints.

Per public endpoint, the exporter should:

- Open one WebSocket connection.
- Subscribe to new best heads.
- Subscribe to finalized heads for later finality work.
- Mark `rpc_up=0` on disconnect, subscription stall, or timeout.
- Poll `system_health` every 30 seconds.
- Poll `state_getRuntimeVersion` and `system_version` every 60 seconds.
- Reconnect with backoff.

It exposes:

- `GET /metrics` for Prometheus.
- `GET /healthz` for a coarse producing or not-producing verdict.
- `GET /status.json` for a cacheable public status payload.

`/status.json` should be component-shaped from the start:

```json
{
  "relay": {
    "status": "up",
    "drift": 8,
    "providers": {}
  }
}
```

System chains can be added later without changing the public payload shape.

## Metrics

Labels: `provider`, `endpoint`, `chain="relay"`.

| Metric | Type | Meaning |
| --- | --- | --- |
| `paseo_rpc_up` | gauge | Endpoint connection and subscription state |
| `paseo_best_block` | gauge | Best block number |
| `paseo_finalized_block` | gauge | Finalized block number |
| `paseo_block_drift_seconds` | gauge | Seconds since the current best head arrived |
| `paseo_finality_lag` | gauge | Best block minus finalized block |
| `paseo_peers` | gauge | Peer count from `system_health` |
| `paseo_is_syncing` | gauge | Sync state from `system_health` |
| `paseo_spec_version` | gauge | Runtime spec version |
| `paseo_expected_block_time_ms` | gauge | Runtime-declared expected block time |
| `paseo_scrape_errors_total` | counter | RPC and subscription errors |
| `paseo_exporter_build_info` | gauge | Exporter version and commit labels |

## Recording Rules

```promql
paseo_relay:degraded_threshold:seconds =
  5 * max(paseo_expected_block_time_ms) / 1000

paseo_relay:stall_threshold:seconds =
  10 * max(paseo_expected_block_time_ms) / 1000

paseo_relay:producing:bool =
  paseo_relay:block_drift:min <= bool scalar(paseo_relay:degraded_threshold:seconds)

paseo_relay:producing:ratio_rate5m =
  avg_over_time(paseo_relay:producing:bool[5m])

paseo_relay:producing:ratio_rate30m =
  avg_over_time(paseo_relay:producing:bool[30m])

paseo_relay:producing:ratio_rate1h =
  avg_over_time(paseo_relay:producing:bool[1h])

paseo_relay:producing:ratio_rate6h =
  avg_over_time(paseo_relay:producing:bool[6h])

paseo_relay:producing:ratio_rate3d =
  avg_over_time(paseo_relay:producing:bool[3d])

paseo_relay:availability:ratio_30d =
  avg_over_time(paseo_relay:producing:bool[30d])
```

Blindness is bad time. If all endpoints are down or the exporter is absent,
availability must not score as healthy.

## Alerts

Hard liveness:

```yaml
- alert: RelayBlockProductionStalled
  expr: paseo_relay:block_drift:min > scalar(paseo_relay:stall_threshold:seconds)
  labels: { severity: page, slo: relay-availability }
  annotations:
    summary: "Relay block production stalled across all healthy vantage points"

- alert: RelayBlockProductionDegraded
  expr: paseo_relay:block_drift:min > scalar(paseo_relay:degraded_threshold:seconds)
  for: 5m
  labels: { severity: ticket, slo: relay-availability }
```

Burn rate:

```yaml
- alert: RelayProductionFastBurn
  expr: |
    (1 - paseo_relay:producing:ratio_rate1h) > (14.4 * 0.005)
    and
    (1 - paseo_relay:producing:ratio_rate5m) > (14.4 * 0.005)
  labels: { severity: page, slo: relay-availability }

- alert: RelayProductionSlowBurn
  expr: |
    (1 - paseo_relay:producing:ratio_rate6h) > (6 * 0.005)
    and
    (1 - paseo_relay:producing:ratio_rate30m) > (6 * 0.005)
  labels: { severity: ticket, slo: relay-availability }

- alert: RelayProductionBudgetBurn3d
  expr: (1 - paseo_relay:producing:ratio_rate3d) > (1 * 0.005)
  labels: { severity: ticket, slo: relay-availability }
```

Monitor-the-monitor:

```yaml
- alert: RelayMonitorBlind
  expr: |
    count(paseo_rpc_up == 1) == 0
    or absent(paseo_block_drift_seconds)
  for: 2m
  labels: { severity: page, slo: relay-availability }
  annotations:
    summary: "Relay monitoring has no healthy vantage points"

- alert: PaseoExporterDown
  expr: up{job="paseo-chain-exporter"} == 0
  for: 2m
  labels: { severity: page }

- alert: RelayExpectedBlockTimeChanged
  expr: changes(max(paseo_expected_block_time_ms)[1h:]) > 0
  labels: { severity: ticket, slo: relay-availability }
  annotations:
    summary: "babe.expectedBlockTime changed; confirm the runtime timing change"
```

Route `severity=page` to the P0 Matrix room and `severity=ticket` to the normal
alerts room through an Alertmanager webhook bridge.

## Dashboard Panels

- Best and finalized height per provider.
- Block drift per provider plus the quorum minimum.
- Degraded and stall threshold lines.
- Provider availability heatmap from `rpc_up`.
- 30 day SLO availability and error budget.
- Peer count, sync state, spec version, and expected block time.

## Stack

| Service | Role |
| --- | --- |
| `paseo-chain-exporter` | Public RPC prober and `/status.json` source |
| `prometheus` | Scrape, recording rules, alert rules |
| `alertmanager` | Routing, deduplication, silences |
| `matrix-bridge` | Alertmanager webhook to Matrix |
| `grafana` | Operator dashboards |

Use a 15 second Prometheus scrape and evaluation interval. Keep 90 days of
retention so the 30 day SLO has history.

Run the stack outside the failure domain it monitors.

## Phases

Phase 0: public RPC relay block production monitoring.

Build the exporter, Prometheus rules, Matrix alerts, Grafana dashboards, and
the 30 day availability calculation.

Phase 1: own relay node scrape.

Scrape native `:9615` metrics from one or two r0gue relay full nodes. Use this
as a corroborating vantage point. A local node falling behind must not page by
itself.

Phase 2: finality.

Add finalized-head drift, GRANDPA metrics, and finality alerts. The exporter
already records finalized height and finality lag.

Phase 3: public status page.

Expose status from `/status.json` or a static export of it. The public page
must not query internal Prometheus directly.

## Build Order

1. Scaffold the compose stack with named Prometheus and Grafana volumes.
2. Build `paseo-chain-exporter` with `prom-client` and RPC-only probes.
3. Add `/metrics`, `/healthz`, and `/status.json`.
4. Add Prometheus scrape config, recording rules, and alert rules.
5. Configure Alertmanager and the Matrix bridge.
6. Provision the Grafana datasource and dashboards.
7. Inject provider drift past 30 seconds and 60 seconds to test ticket and page alerts.
8. Kill the exporter and confirm blindness alerts fire.
9. Deploy to the chosen host and watch a full day before treating the SLO as active.

## Open Questions

1. Where should the stack run?
2. Which Matrix rooms and access token should Alertmanager use?
3. Should Grafana stay internal while the public surface is only the status page?
4. Is a 15 second scrape interval acceptable, or do we want 10 seconds?
5. Which status page approach should we use once `/status.json` is stable?
