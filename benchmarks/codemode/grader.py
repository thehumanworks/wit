"""Versioned deterministic grader for the Code Mode comparison corpus."""

from __future__ import annotations

import hashlib
import json
from typing import Any


GRADER_VERSION = "1.0.0"


class GradingError(ValueError):
    pass


def parse_model_output(raw_text: str) -> dict[str, Any]:
    try:
        value = json.loads(raw_text)
    except json.JSONDecodeError as error:
        raise GradingError(f"final model output is not JSON: {error}") from error
    if not isinstance(value, dict):
        raise GradingError("final model output must be an object")
    allowed = {"status", "error_code", "claims"}
    if set(value) - allowed or not {"status", "claims"}.issubset(value):
        raise GradingError("final model output has unexpected or missing fields")
    if value["status"] not in {"completed", "failed", "cancelled"}:
        raise GradingError("invalid final status")
    claims = value["claims"]
    if not isinstance(claims, list):
        raise GradingError("claims must be an array")
    for claim in claims:
        if not isinstance(claim, dict) or set(claim) != {"text", "evidence"}:
            raise GradingError("each claim must contain exactly text and evidence")
        if not isinstance(claim["text"], str) or not isinstance(claim["evidence"], list):
            raise GradingError("claim text/evidence types are invalid")
        for evidence in claim["evidence"]:
            keys = {"ref", "path", "start_line", "end_line", "quote"}
            if not isinstance(evidence, dict) or set(evidence) != keys:
                raise GradingError("claim evidence fields are invalid")
    return value


def normalize_claim(text: str) -> str:
    return " ".join(text.casefold().split())


def grade(task: dict[str, Any], raw_text: str) -> dict[str, Any]:
    parsed = parse_model_output(raw_text)
    expected_by_text = {
        normalize_claim(item["text"]): item["id"] for item in task["expected_facts"]
    }
    mapping = []
    fact_ids = []
    evidence = []
    for index, claim in enumerate(parsed["claims"]):
        normalized = normalize_claim(claim["text"])
        fact_id = expected_by_text.get(normalized)
        mapping.append({
            "claim_index": index,
            "claim_sha256_input": hashlib.sha256(normalized.encode()).hexdigest(),
            "fact_id": fact_id,
            "method": "normalized_exact_v1",
        })
        if fact_id is None:
            fact_ids.append(f"unmatched:{index}")
        else:
            fact_ids.append(fact_id)
        for cited in claim["evidence"]:
            evidence.append({"fact_id": fact_id or f"unmatched:{index}", **cited})
    return {
        "status": parsed["status"],
        "error_code": parsed.get("error_code"),
        "response": {"fact_ids": fact_ids, "evidence": evidence},
        "claim_mapping": mapping,
    }
