"""Read only provider-announced reset times from HTTP rate-limit responses."""
from email.utils import parsedate_to_datetime
import math


def reset_ms(error, now):
    response = getattr(error, "response", None)
    if getattr(response, "status_code", None) != 429:
        return None
    headers = response.headers
    try:
        if "retry-after" in headers:
            value = headers["retry-after"]
            try:
                reset = now + float(value)
            except ValueError:
                reset = parsedate_to_datetime(value).timestamp()
        elif "x-ratelimit-reset" in headers:
            reset = float(headers["x-ratelimit-reset"])
        else:
            return None
        if not math.isfinite(reset) or reset <= now or reset * 1000 >= 2**64:
            return None
        return math.ceil(reset * 1000)
    except (ValueError, TypeError, OverflowError):
        return None
