"""INSTALL_GUIDE.md -> INSTALL_GUIDE.txt (plain text for people without a Markdown viewer). Used by make_friend_package.ps1."""
import re
import sys


def convert(md):
    out = []
    for ln in md.splitlines():
        if ln.strip().startswith("```") or re.match(r"^\|[-| ]+\|$", ln.strip()):
            continue
        if ln.startswith("|"):
            cells = [c.strip() for c in ln.strip().strip("|").split("|")]
            ln = "  * " + cells[0] + "  ->  " + " | ".join(cells[1:])
        m = re.match(r"^(#+)\s+(.*)", ln)
        if m:
            t = m.group(2).replace("**", "").replace("`", "")
            out += [t.upper() if len(m.group(1)) == 1 else t, ("=" if len(m.group(1)) == 1 else "-") * len(t)]
            continue
        if ln.strip() == "---":
            ln = ""
        ln = ln.replace("**", "").replace("`", "")
        out.append(re.sub(r"(?<!\w)\*([^*]+)\*(?!\w)", r"\1", ln))
    return "\n".join(out) + "\n"


if __name__ == "__main__":
    src, dst = sys.argv[1], sys.argv[2]
    open(dst, "w", encoding="utf-8", newline="\r\n").write(convert(open(src, encoding="utf-8").read()))
