# HPA OpenTelemetry deployment

```bash
docker compose -f deploy/otel/compose.yaml up -d
export HPA_OTEL_ENABLED=1
export HPA_OTEL_ENDPOINT=http://127.0.0.1:4318
export HPA_OTEL_SAMPLE_RATIO=0.1
hpa serve --port 8787
```

Grafana: `http://127.0.0.1:3000` (`admin` / `change-me`, change outside local demo).  
Prometheus: `http://127.0.0.1:9090`. Tempo/Loki back the Grafana trace/log views.

The example is single-node and development-oriented. Production must replace filesystem
retention with managed/HA storage, set credentials, export only approved resource attributes,
and review sampling/cardinality. The example tail-samples errors, traces slower than five
seconds, and a 10% baseline, while deleting known credential and database-query attributes.
For multiple collectors, route all spans for a trace ID to the same tail-sampling instance.
See [OTLP production guide](../../docs/08-platform/otlp.md).
