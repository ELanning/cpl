module pricinghost

go 1.21

require pricing v0.0.0

// Written by `cpl --project .. build --target go`.
replace pricing => ../.cpl/build/go
