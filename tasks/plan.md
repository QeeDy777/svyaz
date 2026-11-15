# Implementation

1. Prove protocol validation and strict strategy parsing with assertion checks.
2. Implement verified package download, bounded extraction and transactional selection.
3. Add Windows process ownership, UI, tray, persistent state and logon task.
4. Check full package compatibility, failure paths, build and package Windows binaries.

Primary risks: absent/stale voice endpoint, upstream BAT format change, missing
GitHub digest, failed update or process start. Each must preserve existing data,
show an explicit error, and never report an unperformed check as successful.
