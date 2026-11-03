"""Measure every TL3 worker prompt as the model receives it (#648).

Usage: python measure_prompts.py CHECKOUT OUTPUT.json

Run inside the checkout's workers environment, with TIKTOKEN_CACHE_DIR holding
o200k_base. For each role it renders the system prompt the way
runtime.build_agent assembles it (role prompt, forcing and rehearsal guidance,
generation instruction, ONLY_JSON and the reply schema with code-only fields
removed, and the maker's model-facing proposal schema), and the tool
definitions the model is offered. It also measures the judge instructions,
the retro prompt and the inline turn messages. Counts use o200k_base, the
tokenizer TL3's forecasts use, as a model-agnostic estimate.
"""

from copy import deepcopy
import json
from pathlib import Path
import sys
import tempfile


def maker_proposal(schema):
    proposal = deepcopy(schema("maker-reply"))
    for variant in proposal["oneOf"]:
        variant["properties"].pop("budget_exhaustion", None)
    done = next(v for v in proposal["oneOf"] if v["properties"]["kind"]["const"] == "done")
    done["required"].remove("head")
    for name in ("head", "test_evidence", "generation_evidence", "checking_handoff"):
        done["properties"].pop(name, None)
    needs = next(v for v in proposal["oneOf"] if v["properties"]["kind"]["const"] == "needs_files")
    needs["properties"].pop("fits", None)
    blocked = next(v for v in proposal["oneOf"] if v["properties"]["kind"]["const"] == "blocked")
    for cited in blocked["properties"]["evidence"]["oneOf"]:
        for name in ("tool", "class"):
            cited["properties"].pop(name, None)
            if name in cited["required"]:
                cited["required"].remove(name)
    return proposal


def main(checkout, output):
    sys.path.insert(0, str(Path(checkout, "workers/src")))
    sys.path.insert(0, str(Path(checkout, "harness/src")))
    from langchain_core.utils.function_calling import convert_to_openai_tool
    from tl3_workers import judges, runtime
    from tl3_workers.contracts import schema
    from tl3_workers.forecast import optional_encoding
    from tl3_workers.roles import ROLES
    from tl3_workers.tools import Worktree, build_tools

    encoding = optional_encoding()
    if encoding is None:
        raise SystemExit("o200k_base is not in TIKTOKEN_CACHE_DIR")

    def tokens(text):
        return len(encoding.encode(text, disallowed_special=()))

    report = {"tokenizer": "o200k_base", "roles": {}, "inline": {}}
    with tempfile.TemporaryDirectory() as scratch, Worktree(scratch) as worktree:
        for name, role in sorted(ROLES.items()):
            prompt_file = Path(checkout, "workers/src/tl3_workers/prompts", role.prompt)
            text = prompt_file.read_text()
            row = {"prompt_words": len(text.split()), "prompt_tokens": tokens(text)}
            if name == "postmortem":
                rendered = judges.PREAMBLE + judges.postmortem_contract(runtime.role_prompt(role))
                row.update(system_tokens=tokens(rendered), schema_tokens=tokens(rendered) - tokens(
                    judges.PREAMBLE + runtime.role_prompt(role)), tool_tokens=0)
            else:
                reply = maker_proposal(schema) if name == "maker" else None
                with_tools = worktree if role.tools else None
                rendered = runtime.role_prompt(role)
                if with_tools is not None and name in {"ingest", "planner", "maker"}:
                    rendered += "\n\n" + worktree.generation_instruction()
                rendered += "\n\n" + runtime.ONLY_JSON + " It must match this schema:\n"
                proposal = deepcopy(schema(role.reply_schema)) if reply is None else reply
                if name == "planner":
                    from tl3_workers.roles.planner import EXPLORATION_DECLARATION
                    proposal = {**EXPLORATION_DECLARATION, **proposal}
                for variant in proposal.get("oneOf", [proposal]):
                    for field in role.code_only_fields:
                        variant["properties"].pop(field, None)
                for field in role.code_only_fields:
                    proposal.get("$defs", {}).pop(field, None)
                schema_text = json.dumps(proposal, ensure_ascii=False)
                row["schema_tokens"] = tokens(schema_text)
                rendered += schema_text
                tools = [convert_to_openai_tool(tool) for tool in build_tools(worktree, role.tools)]
                row.update(system_tokens=tokens(rendered),
                           tool_tokens=tokens(json.dumps(tools, ensure_ascii=False)),
                           tool_description_tokens=sum(tokens(d) for d in role.tools.values()))
            row["guidance_tokens"] = tokens(runtime.role_prompt(role)) - row["prompt_tokens"]
            row["total_tokens"] = row["system_tokens"] + row["tool_tokens"]
            report["roles"][name] = row
    judge_texts = {"judge_needs_rob": judges.NEEDS_ROB, "judge_grade_template": judges.GRADE,
                   "judge_preamble": judges.PREAMBLE}
    from tl3_workers import exploration, turns
    from tl3_workers.roles import maker
    inline = {**judge_texts, "only_json": runtime.ONLY_JSON, "repair": turns.REPAIR,
              "finish": exploration.FINISH, "resume_note": maker.RESUME_NOTE,
              "builtin_tool_descriptions": json.dumps(runtime.BUILTIN_TOOL_DESCRIPTIONS)}
    try:
        from tl3_workers import retro
        inline["retro_prompt"] = retro.PROMPT
    except ImportError:
        pass
    try:
        from tl3_workers.evidence import forcing_guidance
        inline["forcing_guidance"] = forcing_guidance(1)
        from tl3_workers.forcing_rehearsal import GUIDANCE
        inline["rehearsal_guidance"] = GUIDANCE
    except ImportError:
        pass
    report["inline"] = {key: {"words": len(text.split()), "tokens": tokens(text)} for key, text in inline.items()}
    Path(output).write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    for name, row in report["roles"].items():
        print(f"{name:11} words={row['prompt_words']:5} prompt={row['prompt_tokens']:6} "
              f"system={row['system_tokens']:6} tools={row['tool_tokens']:5} total={row['total_tokens']:6}")


if __name__ == "__main__":
    main(*sys.argv[1:])
