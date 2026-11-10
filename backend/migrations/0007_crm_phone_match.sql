-- Phones now match on their last 10 digits (see crm::items::normalize), so numbers
-- with and without country codes are recognised as the same.
UPDATE crm_multifields
SET normalized = right(regexp_replace(value, '[^0-9]', '', 'g'), 10)
WHERE kind = 'phone';
