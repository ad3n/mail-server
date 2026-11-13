"""Status dari endpoint /api/deliveries/{externalId}/status Stalwart."""

import re

STATUSES = {"queued", "deferred", "accepted", "failed", "removed"}


def external_id(value):
    if not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", value) or value in {".", ".."}:
        raise ValueError("External-ID harus 1–128 huruf ASCII, angka, -, _, atau .; bukan . atau ..")
    return value


def outcome(rows):
    if not isinstance(rows, list) or any(not isinstance(row, dict) for row in rows):
        raise ValueError("Respons tracking tidak berupa daftar status")
    states = []
    for row in rows:
        status = row.get("status")
        if not isinstance(status, str) or status not in STATUSES:
            raise ValueError(f"Status tracking tidak dikenal: {status!r}")
        states.append(status)
    if "failed" in states or "removed" in states:
        return 1, "PENGIRIMAN TIDAK BERHASIL: failed/removed; periksa status tiap penerima."
    if states and all(status == "accepted" for status in states):
        return 0, "ACCEPTED: pengiriman selesai dan diterima tujuan; bukan bukti dibaca atau callback webhook diterima."
    return 2, "Status belum final (queued/deferred) atau belum tercatat."
