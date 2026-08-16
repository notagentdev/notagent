def parse_config(path: str) -> dict:
    with open(path, "r", encoding="utf-8") as handle:
        return {"raw": handle.read()}
