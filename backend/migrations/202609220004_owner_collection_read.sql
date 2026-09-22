-- Existing owners had only collections_write. Reading remains an explicit grant.
INSERT IGNORE INTO space_permission_grants (space_id, account_id, permission_code)
SELECT s.id, s.owner_account_id, 'collections_read'
FROM spaces s
JOIN space_memberships m ON m.space_id = s.id AND m.account_id = s.owner_account_id;
