INSERT INTO schema_info (key, value) VALUES ('schema_version', '11')
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;
