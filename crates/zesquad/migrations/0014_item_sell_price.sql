-- What a vendor pays for an item, copper (zesquad_core::items::Item::sell_price), so the
-- auction overview can find listings below it without reading every document. NULL when it
-- can't be sold or isn't known.

ALTER TABLE items ADD COLUMN sell_price INTEGER;

-- Filled for the items already stored. This repeats zesquad_core::items' rule in SQL, once:
-- the tooltip's price first, then the top-level one, and 0 means it can't be sold. Items
-- looked up from now on get it from there.
UPDATE items SET sell_price = NULLIF(
    COALESCE(
        json_extract(json, '$.preview_item.sell_price.value'),
        json_extract(json, '$.sell_price')
    ),
    0
)
WHERE json IS NOT NULL;
