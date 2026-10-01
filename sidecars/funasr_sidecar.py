#!/usr/bin/env python3
"""VoiceBridge Fun-ASR sidecar.

Speaks JSON lines over stdin/stdout — it opens no network socket at all.

Request : {"id": str, "token": str, "op": "health" | "transcribe",
           "audio_path": str, "hotwords": [str], "language": str}
Response: {"id": str, "ok": bool, "text": str, "error": str}

The session token comes from the VOICEBRIDGE_SESSION_TOKEN environment
variable set by the main application; requests with another token are refused.
Only short error categories are written to stderr — never transcripts or
audio paths' contents.
"""

import argparse
import json
import os
import sys


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def reply(payload: dict) -> None:
    sys.stdout.write(json.dumps(payload, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def load_model(model_dir: str, device: str):
    # Imported lazily so a missing dependency produces a clear error line.
    from funasr import AutoModel

    kwargs = dict(model=model_dir, trust_remote_code=True, device=device, disable_update=True)
    remote_code = os.path.join(model_dir, "model.py")
    if os.path.isfile(remote_code):
        kwargs["remote_code"] = remote_code
    return AutoModel(**kwargs)


def transcribe(model, request: dict) -> str:
    kwargs = dict(input=[request["audio_path"]], cache={}, batch_size=1, itn=True)
    hotwords = request.get("hotwords") or []
    if hotwords:
        kwargs["hotwords"] = hotwords
    language = request.get("language")
    if language:
        kwargs["language"] = language
    result = model.generate(**kwargs)
    if not result:
        return ""
    return str(result[0].get("text", "")).strip()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True, help="local Fun-ASR model directory")
    parser.add_argument("--device", default=os.environ.get("VOICEBRIDGE_ASR_DEVICE", "cpu"))
    args = parser.parse_args()

    expected_token = os.environ.get("VOICEBRIDGE_SESSION_TOKEN", "")
    if not expected_token:
        log("error: missing session token")
        return 2

    # Keep stdout clean for the protocol: libraries print to stderr instead.
    protocol_out = sys.stdout
    sys.stdout = sys.stderr
    try:
        model = load_model(args.model, args.device)
    except Exception as exc:  # noqa: BLE001
        log(f"error: model load failed: {type(exc).__name__}")
        return 3
    finally:
        sys.stdout = protocol_out
    log("model loaded")

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        request_id = ""
        try:
            request = json.loads(line)
            request_id = str(request.get("id", ""))
            if request.get("token") != expected_token:
                reply({"id": request_id, "ok": False, "error": "unauthorized"})
                continue
            op = request.get("op")
            if op == "health":
                reply({"id": request_id, "ok": True, "text": ""})
            elif op == "transcribe":
                sys.stdout = sys.stderr
                try:
                    text = transcribe(model, request)
                finally:
                    sys.stdout = protocol_out
                reply({"id": request_id, "ok": True, "text": text})
            else:
                reply({"id": request_id, "ok": False, "error": "unknown_op"})
        except Exception as exc:  # noqa: BLE001
            sys.stdout = protocol_out
            log(f"error: request failed: {type(exc).__name__}")
            reply({"id": request_id, "ok": False, "error": type(exc).__name__})
    return 0


if __name__ == "__main__":
    sys.exit(main())
