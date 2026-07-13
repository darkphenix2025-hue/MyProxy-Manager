#!/usr/bin/env python3
import sqlite3, json, os, shutil, time

db_path = "/home/node/.antigravity_tools/proxy_logs.db"
dest = "/tmp/db_check/proxy_logs.db"
os.makedirs("/tmp/db_check", exist_ok=True)
shutil.copy(db_path, dest)

conn = sqlite3.connect(dest)
c = conn.cursor()

# Query for 500 errors around 2026-06-04 01:36:49
start = int(time.mktime(time.strptime("2026-06-04 01:36:00", "%Y-%m-%d %H:%M:%S"))) * 1000
end = int(time.mktime(time.strptime("2026-06-04 01:37:30", "%Y-%m-%d %H:%M:%S"))) * 1000

rows = c.execute(
    "SELECT id, timestamp, url, status, request_body, upstream_request_body, upstream_response_body, mapped_model, provider_name "
    "FROM request_logs WHERE status = 500 AND timestamp BETWEEN ? AND ?",
    (start, end)
).fetchall()

for r in rows:
    print(f"=== Record ===")
    print(f"id={r[0]}")
    print(f"ts={time.strftime('%Y-%m-%d %H:%M:%S', time.gmtime(r[1]/1000))}")
    print(f"url={r[2]}")
    print(f"status={r[3]}")
    print(f"mapped_model={r[7]}")
    print(f"provider_name={r[8]}")
    print(f"req_body_len={len(str(r[4]) or '')}")
    print(f"up_req_body_len={len(str(r[5]) or '')}")
    print(f"up_resp_body_len={len(str(r[6]) or '')}")

    # Save full bodies to files
    if r[4]:
        with open("/tmp/db_check/original_request.json", "w") as f:
            f.write(r[4])
    if r[5]:
        with open("/tmp/db_check/provider_request.json", "w") as f:
            f.write(r[5])
    if r[6]:
        with open("/tmp/db_check/provider_response.json", "w") as f:
            f.write(r[6])
