"""Check the source evidence for ingest's same-repository merge prerequisites."""


import re


class MergePrerequisiteError(ValueError):
    message = "Each merge prerequisite needs its number and a verbatim quote from the issue"

    def __init__(self):
        super().__init__(self.message)


def check_prerequisites(inputs, reply):
    issue = inputs["issue"]
    sources = [issue.get("body", ""), issue.get("title", "")]
    sources.extend(comment.get("body", "") for comment in issue.get("comments", []))
    seen = set()
    for prerequisite in reply.get("merge_prerequisites", []):
        quote = prerequisite["source_quote"]
        key = prerequisite["kind"], prerequisite["number"]
        if key in seen or not any(quote in (source or "") for source in sources):
            raise MergePrerequisiteError()
        # Prevent a quote for #12 from being used as evidence for #1.
        if not re.search(r"#" + str(prerequisite["number"]) + r"(?!\d)", quote):
            raise MergePrerequisiteError()
        seen.add(key)
