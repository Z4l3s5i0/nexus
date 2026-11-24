import json
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


def metric_sample(
    timestamp_ms,
    experiment_id,
    node_id,
    network,
    client,
    source,
    name,
    value,
    labels=None,
):
    return {
        "timestamp_ms": timestamp_ms,
        "experiment_id": experiment_id,
        "node_id": node_id,
        "network": network,
        "client": client,
        "source": source,
        "name": name,
        "value": value,
        "labels_json": json.dumps(labels, sort_keys=True) if labels is not None else None,
    }


def ingest_samples(api_url, token, samples, timeout=10):
    if not samples:
        return 0
    if not token:
        raise ValueError("METRICS_INGEST_TOKEN is required to store samples")

    endpoint = api_url.rstrip("/") + "/api/metrics/ingest"
    accepted = 0
    for offset in range(0, len(samples), 512):
        batch = samples[offset : offset + 512]
        request = Request(
            endpoint,
            data=json.dumps({"samples": batch}, allow_nan=False).encode("utf-8"),
            headers={
                "Authorization": f"Bearer {token}",
                "Content-Type": "application/json",
            },
            method="POST",
        )
        try:
            with urlopen(request, timeout=timeout) as response:
                result = json.loads(response.read().decode("utf-8"))
        except (HTTPError, URLError, TimeoutError) as exc:
            raise RuntimeError(f"metrics ingestion request failed: {exc}") from exc
        accepted += int(result.get("accepted", 0))
    return accepted