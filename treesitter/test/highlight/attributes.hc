-- An attribute's key is metadata about the definition after it, painted as
-- its own thing; its value is the literal it is, and a struct's labels are
-- the fields they are.

@deprecated "use nat::add"
-- <- @attribute
--          ^^^^^^^^^^^^^^ @string
@since 2n
-- <- @attribute
--     ^^ @number
@stability #Experimental
-- <- @attribute
--         ^^^^^^^^^^^^^ @constructor
@js { from: "host/math", "export as": "add", 0: true }
-- <- @attribute
--    ^^^^ @property
--                       ^^^^^^^^^^^ @property
--                                           ^ @property
--                                              ^^^^ @boolean
let add = fn a b => a

extern register : fn(@async Callback) -> @async Result = "host.register"
--                   ^^^^^^ @attribute
--                          ^^^^^^^^ @type
--                                              ^^^^^^ @type
