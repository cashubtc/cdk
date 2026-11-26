REVOKE ALL ON FUNCTION public.increment_keyset_counter(TEXT, BIGINT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.increment_keyset_counter(TEXT, BIGINT) TO authenticated, service_role;
REVOKE ALL ON FUNCTION public.advance_keyset_counter(TEXT, BIGINT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.advance_keyset_counter(TEXT, BIGINT) TO authenticated, service_role;
