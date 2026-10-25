CREATE FUNCTION public.preserve_terminal_transaction()
RETURNS TRIGGER
LANGUAGE plpgsql
SET search_path = ''
AS $body$
BEGIN
    IF OLD.status != 'pending' AND NEW.status != OLD.status THEN
        RETURN NULL;
    END IF;
    RETURN NEW;
END;
$body$;
