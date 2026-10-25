CREATE TRIGGER preserve_terminal_transaction
    BEFORE UPDATE ON public.transactions
    FOR EACH ROW EXECUTE FUNCTION public.preserve_terminal_transaction();

INSERT INTO public.schema_info (key, value) VALUES ('schema_version', '11')
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;
