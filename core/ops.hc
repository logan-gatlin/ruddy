trait Add: t =
	let [+]: t -> t -> t
end

trait Subtract: t =
	let [-]: t -> t -> t
end

trait Divide: t =
	let [/]: t -> t -> t
end

trait Multiply: t =
	let [*]: t -> t -> t
end

trait And: t =
	let [and]: t -> t -> t
end

trait Or: t =
	let [or]: t -> t -> t
end

trait Xor: t =
	let [xor]: t -> t -> t
end

trait Not: t =
	let [not]: t -> t
end

trait Negate: t =
	let [~]: t -> t
end

trait Equal: t =
	let [==]: t -> t -> Boolean
end

type Order =
	| Less
	| Equal
	| Greater

trait Order: t =
	let compare: t -> t -> Order
end

let [<] = fn a b => match Order::compare a b with
	| Order::Less => true
	| _ => false

let [<=] = fn a b => match Order::compare a b with
	| Order::Greater => false
	| _ => true

let [>] = fn a b => match Order::compare a b with
	| Order::Greater => true
	| _ => false

let [>=] = fn a b => match Order::compare a b with
	| Order::Less => false
	| _ => true


let [|>] = fn a b => b a
let [>>] = fn a b c => b (a c)
let [<<] = fn a b c => a (b c)
let [!=] = Equal::[==] >> Not::[not]
