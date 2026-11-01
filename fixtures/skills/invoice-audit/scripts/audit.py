import argparse
import json
import os


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    with open(args.input, "r", encoding="utf-8") as handle:
        payload = json.load(handle)

    items = payload.get("items", [])
    total = round(sum(float(item.get("amount", 0)) for item in items), 2)
    result = {
        "ok": True,
        "invoiceId": payload.get("invoiceId", ""),
        "total": total,
        "itemCount": len(items),
    }

    os.makedirs(args.output, exist_ok=True)
    with open(os.path.join(args.output, "summary.json"), "w", encoding="utf-8") as handle:
        json.dump(result, handle, sort_keys=True)
        handle.write("\n")

    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
