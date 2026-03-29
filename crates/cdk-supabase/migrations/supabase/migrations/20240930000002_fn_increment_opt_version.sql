-- Trigger function: bump opt_version on every UPDATE for optimistic locking
CREATE OR REPLACE FUNCTION increment_opt_version()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $body$
begin
  NEW.opt_version = COALESCE(OLD.opt_version, 0) + 1;
  RETURN NEW;
end
$body$;
