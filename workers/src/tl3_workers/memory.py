"""Bounded run evidence for the controller. No Hindsight client lives in workers."""

from contextvars import ContextVar
import json
import os
from pathlib import Path
from threading import Lock
from uuid import uuid4


current = ContextVar("tl3_memory", default=None)


def bounded(value, depth=6):
    if depth == 0:
        return "[truncated]"
    if isinstance(value, str):
        return value[:1000] + (" [truncated]" if len(value) > 1000 else "")
    if isinstance(value, dict):
        return {str(key)[:100]: bounded(item, depth - 1) for key, item in list(value.items())[:24]}
    if isinstance(value, list):
        return [bounded(item, depth - 1) for item in value[:24]]
    return value


class Summary:
    def __init__(self, request):
        path = request.get("memory", {}).get("summary_file")
        self.path = Path(path) if path else None
        self.lock = Lock()
        self.value = {"activity": {"files_read": [], "files_changed": [], "tests": []},
                      "reply": None, "outcome": "running", "reason": None}

    def save(self):
        if self.path is None:
            return
        temporary = self.path.with_name(self.path.name + "." + uuid4().hex + ".tmp")
        try:
            value = {**self.value, "reply": bounded(self.value["reply"])}
            encoded = json.dumps(value, ensure_ascii=False, allow_nan=False).encode("utf-8")
            if len(encoded) > 24000:
                value["reply"] = {"summary": encoded[:2000].decode("utf-8", errors="ignore"), "truncated": True}
                encoded = json.dumps(value, ensure_ascii=False, allow_nan=False).encode("utf-8")
            with temporary.open("xb") as handle:
                handle.write(encoded)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary, self.path)
            directory = os.open(self.path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
        except (OSError, ValueError, TypeError):
            # The controller can reconstruct a fallback from request, reply and exit.
            pass
        finally:
            try:
                temporary.unlink(missing_ok=True)
            except OSError:
                pass

    def tool(self, name, args, result, error=False):
        with self.lock:
            activity = self.value["activity"]
            if name in {"read", "write", "edit"} and not error:
                field = "files_read" if name == "read" else "files_changed"
                path = args.get("path")
                if isinstance(path, str) and path not in activity[field] and len(activity[field]) < 32:
                    activity[field].append(path[:256])
            if name == "run_tests" and len(activity["tests"]) < 16:
                if isinstance(result, str):
                    try:
                        result = json.loads(result)
                    except ValueError:
                        result = {}
                result = result if isinstance(result, dict) else {}
                activity["tests"].append(bounded({
                    "selectors": args.get("tests"), "status": result.get("status", "tool_error" if error else "unknown"),
                    "action_key": result.get("action_key"), "totals": result.get("totals"),
                    "tests": [{"name": item.get("name"), "outcome": item.get("outcome")}
                              for item in result.get("tests", [])[:16]],
                }))
            self.save()

    def finish(self, reply, error):
        with self.lock:
            self.value.update(reply=reply, outcome="failed" if error else "succeeded",
                              reason=type(error).__name__ if error else None)
            self.save()
