"""Word error rate between reference and hypothesis transcripts (no dependencies)."""
import re
import sys


def norm(s: str) -> list[str]:
    s = s.lower().replace("'", "")
    s = re.sub(r"[^a-z0-9 ]+", " ", s)
    return s.split()


def wer(ref: str, hyp: str) -> float:
    r, h = norm(ref), norm(hyp)
    d = list(range(len(h) + 1))
    for i in range(1, len(r) + 1):
        prev, d[0] = d[0], i
        for j in range(1, len(h) + 1):
            cur = d[j]
            d[j] = min(d[j] + 1, d[j - 1] + 1, prev + (r[i - 1] != h[j - 1]))
            prev = cur
    return d[len(h)] / max(len(r), 1)


if __name__ == "__main__":
    print(f"{wer(open(sys.argv[1], encoding='utf-8-sig').read(), open(sys.argv[2], encoding='utf-8-sig').read()):.4f}")
