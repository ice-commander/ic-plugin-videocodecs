"""Copies a library and every library it reaches outside the system into one folder, points them at each
other with @loader_path, signs them again, and refuses any whose Homebrew licence is GPL with no other
choice. Writes what it carried and the licence texts it found to <notes>.

    python3 -I bundle-macos.py <libmpv.2.dylib> <folder> <notes>
"""
import json
import os
import re
import subprocess
import sys

SYSTEM = ("/usr/lib/", "/System/")
CELLAR = re.compile(r"/Cellar/([^/]+)/([^/]+)/")
LICENCE_FILES = re.compile(r"^(COPYING|LICEN[CS]E|COPYRIGHT|NOTICE)", re.IGNORECASE)
# Homebrew licenses a formula as a whole; these libraries are its LGPL part, the GPL part being a command-line tool.
LIBRARY_LICENCE = {
    "libfribidi": "LGPL-2.1-or-later",
    "libintl": "LGPL-2.1-or-later",
}


def run(*command):
    return subprocess.run(command, capture_output=True, text=True, check=True).stdout


def own_id(path):
    lines = run("otool", "-D", path).splitlines()
    return lines[1].strip() if len(lines) > 1 else ""


def linked(path):
    own = own_id(path)
    names = [line.strip().split(" (")[0] for line in run("otool", "-L", path).splitlines()[1:]]
    return [name for name in names if name and name != own and not name.startswith(SYSTEM)]


def rpaths(path):
    return re.findall(r"cmd LC_RPATH\n\s+cmdsize \d+\n\s+path (.+?) \(offset", run("otool", "-l", path))


def resolve(name, by):
    here = os.path.dirname(by)
    if name.startswith(("@loader_path/", "@executable_path/")):
        return os.path.realpath(os.path.join(here, name.split("/", 1)[1]))
    if name.startswith("@rpath/"):
        for searched in rpaths(by):
            candidate = os.path.join(searched.replace("@loader_path", here), name[len("@rpath/"):])
            if os.path.exists(candidate):
                return os.path.realpath(candidate)
        sys.exit(f"{by}: cannot find {name} on its rpaths")
    if os.path.exists(name):
        return os.path.realpath(name)
    # A build prefix that has since moved: its libraries still name the old one, and sit beside each other.
    beside = os.path.join(here, os.path.basename(name))
    if os.path.exists(beside):
        return os.path.realpath(beside)
    sys.exit(f"{by}: links {name}, which is not there")


def gpl_only(expression):
    alternatives = re.split(r"\s+OR\s+", expression.replace("(", " ").replace(")", " "))
    return all(re.search(r"(?<!L)GPL", alternative) for alternative in alternatives)


def main():
    root, folder, notes = os.path.realpath(sys.argv[1]), sys.argv[2], sys.argv[3]
    carried = {}
    queue = [root]
    while queue:
        path = queue.pop()
        if path in carried:
            continue
        carried[path] = os.path.basename(own_id(path) or path)
        queue.extend(resolve(name, path) for name in linked(path))

    names = list(carried.values())
    if len(set(names)) != len(names):
        sys.exit(f"two libraries would land under one name: {sorted(names)}")

    os.makedirs(folder, exist_ok=True)
    for path, name in carried.items():
        target = os.path.join(folder, name)
        with open(path, "rb") as source, open(target, "wb") as out:
            out.write(source.read())
        os.chmod(target, 0o755)
        changes = []
        for linked_name in linked(path):
            changes += ["-change", linked_name, "@loader_path/" + carried[resolve(linked_name, path)]]
        for searched in rpaths(path):
            changes += ["-delete_rpath", searched]
        run("install_name_tool", "-id", "@loader_path/" + name, *changes, target)
        run("codesign", "--force", "--sign", "-", target)

    formulae = {}
    for path, name in carried.items():
        found = CELLAR.search(path)
        if found:
            formulae.setdefault(found.group(1), (found.group(2), []))[1].append(name)
    known = json.loads(run("brew", "info", "--json=v2", "--formula", *sorted(formulae)))["formulae"] if formulae else []
    licences = {entry["name"]: entry.get("license") or "" for entry in known}

    def licence_of(path, name):
        found = CELLAR.search(path)
        if not found:
            return ""
        return LIBRARY_LICENCE.get(name.split(".")[0]) or licences.get(found.group(1)) or "no licence declared"

    refused = [f"{name} ({licence_of(path, name)})" for path, name in carried.items()
               if CELLAR.search(path) and (licence_of(path, name) == "no licence declared"
                                           or gpl_only(licence_of(path, name)))]
    if refused:
        for name in os.listdir(folder):
            os.remove(os.path.join(folder, name))
        sys.exit("GPL or undeclared, not carried: " + ", ".join(sorted(refused)))

    brew = run("brew", "--prefix").strip()
    with open(notes, "w") as out:
        for path, name in sorted(carried.items(), key=lambda item: item[1]):
            found = CELLAR.search(path)
            origin = f"Homebrew {found.group(1)} {found.group(2)}, {licence_of(path, name)}" if found else "built by libmpv-macos.sh"
            out.write(f"  {name:32} {origin}\n")
        for formula, (version, _) in sorted(formulae.items()):
            prefix = os.path.join(brew, "Cellar", formula, version)
            for entry in sorted(os.listdir(prefix)):
                if LICENCE_FILES.match(entry) and os.path.isfile(os.path.join(prefix, entry)):
                    with open(os.path.join(prefix, entry), errors="replace") as text:
                        out.write(f"\n\n===== {formula} {version}: {entry} =====\n\n{text.read()}")


main()
