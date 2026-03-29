-- Helper: extract wallet ID from JWT sub claim (falls back to auth.uid())
CREATE OR REPLACE FUNCTION public.get_current_wallet_id()
RETURNS text
LANGUAGE sql
STABLE
SECURITY DEFINER
AS $body$
  SELECT COALESCE(
    nullif(current_setting('request.jwt.claims', true)::json->>'sub', ''),
    auth.uid()::text
  )
$body$;
