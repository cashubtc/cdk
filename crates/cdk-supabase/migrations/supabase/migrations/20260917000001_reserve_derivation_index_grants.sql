REVOKE ALL ON FUNCTION public.reserve_derivation_index(TEXT, BIGINT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.reserve_derivation_index(TEXT, BIGINT) TO authenticated, service_role;
