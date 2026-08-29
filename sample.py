"""A small self-contained sample module used for testing.

It provides a few tiny helpers around text processing so that scripts,
linters, or simple test runs have something concrete to operate on.
"""

from __future__ import annotations

from collections import Counter
from dataclasses import dataclass, field
from typing import Iterable


@dataclass
class TextStats:
    """Aggregate statistics computed over a piece of text."""

    characters: int = 0
    words: int = 0
    lines: int = 0
    most_common: list = field(default_factory=list)

    def to_summary(self) -> str:
        """Return a single human-readable summary line."""
        common = ", ".join(f"{word}={n}" for word, n in self.most_common)
        return (
            f"{self.lines} lines, {self.words} words, "
            f"{self.characters} characters; top: {common}"
        )


def _tokens(text: str) -> Iterable[str]:
    """Yield lowercase alphanumeric token runs from a string."""
    buffer = ""
    for char in text:
        if char.isalnum():
            buffer += char.lower()
        elif buffer:
            yield buffer
            buffer = ""
    if buffer:
        yield buffer


def compute_stats(text: str) -> TextStats:
    """Compute character, word and line statistics for ``text``."""
    stats = TextStats()
    stats.characters = len(text)
    stats.lines = text.count("\n") + (1 if text.strip() else 0)
    words = list(_tokens(text))
    stats.words = len(words)
    counter = Counter(words)
    stats.most_common = counter.most_common(5)
    return stats


def reverse_lines(text: str) -> str:
    """Return ``text`` with the order of its lines reversed."""
    lines = text.splitlines()
    lines.reverse()
    return "\n".join(lines)


def dedupe_keep_order(items: Iterable[str]) -> List[str]:
    """Remove duplicate strings while preserving the first occurrence."""
    seen: set[str] = set()
    result: List[str] = []
    for item in items:
        if item not in seen:
            seen.add(item)
            result.append(item)
    return result


def _run_demo() -> None:
    """Print a small demonstration when the module is executed."""
    sample = (
        "the quick brown fox\n"
        "the lazy dog\n"
        "the quick fox again\n"
    )
    stats = compute_stats(sample)
    print("Original:")
    print(sample.rstrip())
    print()
    print("Reversed:")
    print(reverse_lines(sample).rstrip())
    print()
    print("Stats:", stats.to_summary())


if __name__ == "__main__":
    _run_demo()
