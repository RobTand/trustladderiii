"""Match complete assertion evidence before truncating its display text."""
import ast
import dis
from enum import StrEnum
import io
import tokenize


class RedMessageMatch(StrEnum):
    MESSAGE = 'message'
    SOURCE_ASSERTION = 'source-assertion'
    MISMATCH = 'mismatch'


def rewritten_assertion(error):
    trace = error.__traceback__
    if not isinstance(error, AssertionError) or trace is None:
        return None
    while trace.tb_next is not None:
        trace = trace.tb_next
    code = trace.tb_frame.f_code
    instruction = next((item for item in dis.get_instructions(code) if item.offset == trace.tb_lasti), None)
    if instruction is None or instruction.opname != 'RAISE_VARARGS' or '@pytest_ar' not in code.co_names:
        return None
    try:
        with tokenize.open(code.co_filename) as file:
            source = file.read()
        tree = ast.parse(source)
        position = instruction.positions
        statement = next((node for node in ast.walk(tree) if isinstance(node, ast.Assert)
                          and (node.lineno, node.col_offset, node.end_lineno, node.end_col_offset)
                          == (position.lineno, position.col_offset, position.end_lineno, position.end_col_offset)), None)
        return (statement, ast.get_source_segment(source, statement)) if statement else None
    except (OSError, SyntaxError, UnicodeError):
        return None


def assertion_reason(error, assertion):
    reason = str(error)
    if assertion is not None and assertion[0].msg is not None:
        # Only rewritten explicit messages have pytest's explanation boundary.
        # Manual AssertionError and pytest.fail keep their entire message.
        message, boundary, _ = reason.partition('\nassert ')
        if boundary:
            reason = message.replace('\n  ', '\n')
    return reason


def source_tokens(source):
    # Ignore formatting whitespace, preserving token boundaries and every byte
    # inside string literals. A different literal is never the same assertion.
    try:
        tokens = tuple((token.type, token.string) for token in tokenize.generate_tokens(io.StringIO(source).readline)
                       if token.type not in (tokenize.INDENT, tokenize.DEDENT, tokenize.NL,
                                             tokenize.NEWLINE, tokenize.ENDMARKER))
        # Tokenization also consumes continuation backslashes. The raw key
        # ensures that only whitespace can disappear from either declaration.
        return ''.join(source.split()), tokens
    except (tokenize.TokenError, IndentationError):
        return None


def capture(error, declared: str | None) -> tuple[str, RedMessageMatch | None]:
    assertion = rewritten_assertion(error)
    reason = assertion_reason(error, assertion)
    if declared is None:
        return reason, None
    if reason == declared:
        return reason, RedMessageMatch.MESSAGE
    if assertion is not None and assertion[0].msg is None and assertion[1] is not None:
        source = source_tokens(assertion[1])
        if source is not None and source == source_tokens(declared):
            return reason, RedMessageMatch.SOURCE_ASSERTION
    return reason, RedMessageMatch.MISMATCH
