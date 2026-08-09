# Identity-model quota uses the requested logical name

AiProxy admits token quota before it selects an AI service. The client model or
alias is therefore the request-stable `logical_model`, while each candidate
service independently maps that name to an attempt-local provider model.
`identityModel` quota keys must use the requested logical name; there is no
single candidate-independent canonical provider model at admission time.

Consequently, the canonical catalog name and each configured alias are
distinct quota buckets even when one service maps them to the same provider
model. With the schema limit of 32 aliases, one identity can consume a limit
through up to 33 declared names. Operators needing one aggregate cap use the
`identity` scope or avoid aliases on quota-controlled models. `Downgrade`
permits only `identityModel`, so its configured limits must account for aliases.

Do not normalize through the first candidate. That would make quota identity
depend on route order and produce incorrect keys when failover candidates map
the same logical name differently.
