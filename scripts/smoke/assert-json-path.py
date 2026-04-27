#!/usr/bin/env python3
"""Assert a JSON value at a dot path read from stdin."""

from __future__ import annotations

import argparse
import json
import re
import sys
from typing import Any


_PATH_SEGMENT_RE = re.compile(r"([^\[\]]+)|\[(\d+)\]")


class AssertionFailure(Exception):
    """Raised for user-facing assertion failures."""


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Assert a JSON value at a dot path read from stdin.",
    )
    parser.add_argument("--path", required=True, help="Dot path with optional array indexes.")

    assertion_mode = parser.add_mutually_exclusive_group(required=True)
    assertion_mode.add_argument("--equals", help="Expected JSON literal, or raw string.")
    assertion_mode.add_argument("--is-null", action="store_true", help="Assert value is null.")
    assertion_mode.add_argument(
        "--is-not-null",
        action="store_true",
        help="Assert value exists and is not null.",
    )

    return parser.parse_args()


def _load_stdin_json() -> Any:
    try:
        return json.load(sys.stdin)
    except json.JSONDecodeError as exc:
        raise AssertionFailure(f"failed to parse stdin as JSON: {exc}") from exc


def _parse_path(path: str) -> list[str | int]:
    if not path:
        raise AssertionFailure("--path must not be empty")

    tokens: list[str | int] = []
    for segment in path.split("."):
        if not segment:
            raise AssertionFailure(f"invalid path {path!r}: empty path segment")

        position = 0
        for match in _PATH_SEGMENT_RE.finditer(segment):
            if match.start() != position:
                raise AssertionFailure(f"invalid path segment {segment!r} in {path!r}")
            position = match.end()

            key, index = match.groups()
            if key is not None:
                tokens.append(key)
            else:
                tokens.append(int(index))

        if position != len(segment):
            raise AssertionFailure(f"invalid path segment {segment!r} in {path!r}")

    return tokens


def _format_path_token(token: str | int) -> str:
    if isinstance(token, int):
        return f"[{token}]"
    return token


def _resolve_path(value: Any, path: str) -> Any:
    current = value
    traversed: list[str | int] = []

    for token in _parse_path(path):
        traversed.append(token)
        location = "".join(
            _format_path_token(part) if isinstance(part, int) else f".{part}"
            for part in traversed
        ).lstrip(".")

        if isinstance(token, str):
            if not isinstance(current, dict):
                raise AssertionFailure(
                    f"path {path!r} failed at {location!r}: expected object, "
                    f"found {type(current).__name__}"
                )
            if token not in current:
                raise AssertionFailure(f"path {path!r} failed at {location!r}: key not found")
            current = current[token]
            continue

        if not isinstance(current, list):
            raise AssertionFailure(
                f"path {path!r} failed at {location!r}: expected array, "
                f"found {type(current).__name__}"
            )
        if token >= len(current):
            raise AssertionFailure(
                f"path {path!r} failed at {location!r}: index {token} out of range "
                f"for array of length {len(current)}"
            )
        current = current[token]

    return current


def _parse_expected(value: str) -> Any:
    try:
        return json.loads(value)
    except json.JSONDecodeError:
        return value


def _json_repr(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def _assert_value(actual: Any, args: argparse.Namespace) -> None:
    if args.is_null:
        if actual is not None:
            raise AssertionFailure(f"expected null, found {_json_repr(actual)}")
        return

    if args.is_not_null:
        if actual is None:
            raise AssertionFailure("expected non-null value, found null")
        return

    expected = _parse_expected(args.equals)
    if actual != expected:
        raise AssertionFailure(
            f"expected {_json_repr(expected)}, found {_json_repr(actual)}"
        )


def main() -> int:
    args = _parse_args()
    try:
        document = _load_stdin_json()
        actual = _resolve_path(document, args.path)
        _assert_value(actual, args)
    except AssertionFailure as exc:
        print(f"assert-json-path: {exc}", file=sys.stderr)
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
