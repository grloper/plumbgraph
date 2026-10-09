(import_statement source: (string (string_fragment) @module)) @import
(export_statement source: (string (string_fragment) @module)) @import
(call_expression function: (identifier) @_f arguments: (arguments . (string (string_fragment) @module)) (#eq? @_f "require")) @import
(call_expression function: (import) arguments: (arguments . (string (string_fragment) @module))) @import
