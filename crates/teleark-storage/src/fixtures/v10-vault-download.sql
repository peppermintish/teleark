-- Independent v10 local-output fixture. Unix path bytes encode /tmp/TeleArk/京都.pdf.
INSERT INTO vault_downloaded_files (id, account_id, chat_id, package_id, path_encoding, destination_path, size_bytes, completed_at_unix_ms)
VALUES (42, 7, 90, '5441524b504b4731000000000000002a', 'unix-bytes-v1', X'2f746d702f54656c6541726b2fe4baace983bd2e706466', 1234, 1788739200000);
