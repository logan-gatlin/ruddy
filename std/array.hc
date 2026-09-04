extern len : ['a] -> Nat = "$arrayLen"
extern get : ['a] -> Nat -> Option 'a = "$arrayGet"
extern set : ['a] -> Nat -> 'a -> Option ['a] = "$arraySet"
extern push : ['a] -> 'a -> ['a] = "$arrayPush"
