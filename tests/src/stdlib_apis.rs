use std::{fs, path::Path, process::Command};

fn project(source: &str, platform: &str, kind: &str) -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    let standard = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    fs::write(project.path().join("Ruddy.toml"), format!(
        "name = \"api-test\"\nversion = \"0.1.0\"\nkind = {kind:?}\nroot = \"main.rud\"\ntarget = \"js\"\nplatform = {platform:?}\n\n[dependencies]\nstd = {standard:?}\n"
    )).unwrap();
    fs::write(project.path().join("main.rud"), source).unwrap();
    project
}

/// A project whose manifest binds `Nat` and `Int` to a precision, so what the
/// standard library accepts can be watched at more than the default domain.
fn domain_project(source: &str, integers: u32) -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    let standard = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    fs::write(project.path().join("Ruddy.toml"), format!(
        "name = \"api-test\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = \"js\"\nplatform = \"node\"\nintegers = {integers}\n\n[dependencies]\nstd = {standard:?}\n"
    )).unwrap();
    fs::write(project.path().join("main.rud"), source).unwrap();
    project
}

fn run(project: &Path, script: &str) {
    ruddy_cli::check_project(project).expect("API consumer checks");
    let artifact = ruddy_cli::build_project(project).expect("API consumer builds");
    let script = format!(
        "import assert from 'node:assert/strict'; import {{pathToFileURL}} from 'node:url'; const app = await import(pathToFileURL({}));\n{script}",
        serde_json::to_string(artifact.with_extension("js").to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .current_dir(project)
        .env("RUDDY_API_TEST", "value")
        .env("RUDDY_API_EMPTY", "")
        .env_remove("RUDDY_API_MISSING")
        .args(["--input-type=module", "--eval", &script])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn json_and_url_values_work_on_node_and_web() {
    for platform in ["node", "web"] {
        let project = project(
            r#"
let parse = std::json::parse
let stringify = std::json::stringify
let decode: String -> std::result::Result { name: String, counts: [Nat] } std::json::Error = std::json::decode
let inspect = fn text => match std::json::parse text with
| #Some (#Object fields) => std::array::len fields
| _ => 0n
end
let url = std::url::parse
let resolve = std::url::resolve
let query = std::url::parse_query
let query_string = std::url::stringify_query
let query_get = std::url::get
let query_all = std::url::get_all
let query_set = std::url::set
let query_append = std::url::append
let query_remove = std::url::remove
let with_query: std::url::Url -> std::url::Query -> std::result::Result std::url::Url std::url::Error = std::url::with_query
"#,
            platform,
            "library",
        );
        run(
            project.path(),
            r#"
const some = result => { assert.equal(result.tag, 'Some', JSON.stringify(result)); return result.value; };
const sum = (tag, value) => ({tag, value});
const pair = (a,b) => Object.assign(Object.create(null), {0:a,1:b});
const value = some(await app.parse('{"name":"hé\\n\\\"","counts":[0,3],"nil":null,"ok":true,"__proto__":{"safe":true}}'));
assert.equal(value.tag, 'Object');
const text = some(await app.stringify(value));
assert.deepEqual(JSON.parse(text), JSON.parse('{"name":"hé\\n\\\"","counts":[0,3],"nil":null,"ok":true,"__proto__":{"safe":true}}'));
assert.equal(await app.inspect('{"a":1,"b":2}'), 2);
assert.equal((await app.parse('{')).tag, 'Error');
assert.equal((await app.parse('1e400')).tag, 'Some');
assert.equal((await app.stringify(sum('Number', { token: 'Infinity' }))).tag, 'Error');
assert.equal((await app.stringify(sum('Number', { token: 'NaN' }))).tag, 'Error');
assert.deepEqual({...some(await app.decode('{"name":"Ada","counts":[1,2,3]}'))}, {name:'Ada', counts:[1,2,3]});
const bad = await app.decode('{"name":"Ada","counts":[1,"bad"]}');
assert.equal(bad.tag, 'Error'); assert.equal(bad.value.tag, 'Codec');
assert.deepEqual(bad.value.value.path, [sum('Field', 'counts'), sum('Index', 1)]);
assert.equal(bad.value.value.kind.tag, 'Unexpected');
assert.equal((await app.decode('no')).value.value.kind.tag, 'Unexpected');
const big = Array.from({length: 1100}, (_,i) => i);
const bigValue = some(await app.parse(JSON.stringify(big)));
assert.deepEqual(JSON.parse(some(await app.stringify(bigValue))), big);
assert.equal(JSON.parse(some(await app.stringify(some(await app.parse('{"a":1,"a":2}'))))).a, 2);
const url = some(await app.url('https://EXAMPLE.com:443/a/../b?q=a+b&q=%2B#x'));
assert.equal(url.href, 'https://example.com/b?q=a+b&q=%2B#x'); assert.equal(url.origin, 'https://example.com');
assert.equal(some(await (await app.resolve(url.href))('../c')).pathname, '/c');
assert.equal((await app.url('relative')).tag, 'Error');
assert.equal((await (await app.resolve('bad'))('also-bad')).tag, 'Error');
const query = await app.query(url.search);
assert.deepEqual(query, [pair('q','a b'), pair('q','+')]);
assert.deepEqual(await (await app.query_all('q'))(query), ['a b','+']);
assert.equal(some(await (await app.query_get('q'))(query)), 'a b');
assert.equal((await (await app.query_get('missing'))(query)).tag, 'None');
assert.equal(await app.query_string(query), 'q=a+b&q=%2B');
const changed = await (await (await app.query_set('q'))('hello & world'))(query);
assert.deepEqual(changed, [pair('q','hello & world')]); assert.equal(query.length, 2);
const appended = await (await (await app.query_append('q'))('last'))(changed);
assert.equal(appended.length, 2);
assert.deepEqual(await (await app.query_remove('q'))(appended), []);
assert.equal(some(await (await app.with_query(url))(changed)).href, 'https://example.com/b?q=hello+%26+world#x');
"#,
        );
    }
}

#[test]
fn process_and_path_apis_use_the_node_host() {
    let project = project(
        r#"
let args = std::process::args
let env = std::process::env
let cwd = std::process::cwd
let resolve = std::path::resolve
let relative = std::path::relative
let join = std::path::join
let normalize = std::path::normalize
let basename = std::path::basename
let dirname = std::path::dirname
let extension = std::path::extension
let is_absolute = std::path::is_absolute
let parts = std::path::parse
let posix = std::path::posix::normalize
let windows = std::path::windows::normalize
let windows_join = std::path::windows::join
let windows_parts = std::path::windows::parse
"#,
        "node",
        "library",
    );
    run(
        project.path(),
        r#"
const path = await import('node:path');
const some = result => { assert.equal(result.tag, 'Some', JSON.stringify(result)); return result.value; };
process.argv = ['node', 'script', 'one', 'two'];
assert.deepEqual(await app.args({}), ['one','two']);
assert.equal(some(some(await app.env('RUDDY_API_TEST'))), 'value');
assert.equal(some(some(await app.env('RUDDY_API_EMPTY'))), '');
assert.equal(some(await app.env('RUDDY_API_MISSING')).tag, 'None');
delete process.env.toString;
assert.equal(some(await app.env('toString')).tag, 'None');
const originalEnv = process.env;
process.env = new Proxy({}, {getOwnPropertyDescriptor() { throw new Error('unreadable'); }});
try { assert.equal((await app.env('valid')).value.kind.tag, 'Other'); } finally { process.env = originalEnv; }
for (const name of ['', 'bad=name', 'bad\0name']) assert.equal((await app.env(name)).value.kind.tag, 'InvalidName');
assert.equal(some(await app.cwd({})), process.cwd());
assert.equal(some(await app.resolve(['a','..','b'])), path.resolve('a','..','b'));
assert.equal(some(await (await app.relative('a'))('b')), path.relative('a','b'));
for (const input of ['', '/', '.hidden', '/a/../b/', 'a.tar.gz', '//a//b']) {
  assert.equal(await app.normalize(input), path.normalize(input));
  assert.equal(await app.basename(input), path.basename(input));
  assert.equal(await app.dirname(input), path.dirname(input));
  assert.equal(await app.extension(input), path.extname(input));
  assert.equal(await app.is_absolute(input), path.isAbsolute(input));
  assert.deepEqual({...await app.parts(input)}, path.parse(input));
}
assert.equal(await app.join(['a','','..','b']), path.join('a','','..','b'));
assert.equal(await app.join([]), '.');
assert.equal(await app.posix('/a//../b/'), '/b/');
assert.equal(await app.windows('C:\\a\\..\\b\\'), 'C:\\b\\');
assert.equal(await app.windows_join(['C:\\a', '..', 'b']), 'C:\\b');
assert.deepEqual({...await app.windows_parts('C:\\a\\b.txt')}, path.win32.parse('C:\\a\\b.txt'));
const original = process.cwd;
process.cwd = () => { throw new Error('unavailable'); };
try {
  assert.equal((await app.cwd({})).value.kind.tag, 'Unavailable');
  assert.equal((await app.resolve([])).tag, 'Error');
  assert.equal((await (await app.relative('a'))('b')).tag, 'Error');
} finally { process.cwd = original; }
"#,
    );
}

#[test]
fn http_requests_buffer_bodies_preserve_statuses_and_bound_resources() {
    for platform in ["node", "web"] {
        let project = project(
            r#"
let defaults = std::http::defaults
let request = std::http::request
let get = std::http::get
let post = std::http::post
let text: std::http::Response -> String = std::http::text
let header = std::http::header
let is_success: std::http::Response -> Bool = std::http::is_success
let json: std::http::Response -> std::result::Result { answer: Nat } std::json::Error = std::http::json
"#,
            platform,
            "library",
        );
        run(
            project.path(),
            r#"
const {createServer} = await import('node:http');
const server = createServer(async (req,res) => {
  if (req.url === '/slow') { setTimeout(() => res.end('late'), 200); return; }
  if (req.url === '/slow-body') { res.writeHead(200); res.write('first'); setTimeout(() => res.end('late'), 200); return; }
  if (req.url === '/redirect') { res.writeHead(302, {location:'/json'}); res.end(); return; }
  if (req.url === '/big') { res.end('x'.repeat(1024)); return; }
  if (req.url === '/missing') { res.writeHead(404); res.end('not found'); return; }
  if (req.url === '/json') { res.setHeader('content-type','application/json'); res.end('{"answer":42}'); return; }
  if (req.url === '/binary') { res.end(Buffer.from([0,128,255])); return; }
  const chunks = []; for await (const chunk of req) chunks.push(chunk);
  res.setHeader('x-method', req.method); res.setHeader('x-request', req.headers['x-request'] || 'none');
  res.end(Buffer.concat(chunks));
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const base = `http://127.0.0.1:${server.address().port}`;
const some = result => { assert.equal(result.tag, 'Some', JSON.stringify(result)); return result.value; };
try {
  const response = some(await app.get(base+'/json'));
  assert.equal(await app.is_success(response), true);
  assert.equal(await app.text(response), '{"answer":42}');
  assert.deepEqual({...some(await app.json(response))}, {answer:42});
  assert.equal(some(await (await app.header('Content-Type'))(response.headers)), 'application/json');
  assert.equal((await (await app.header('missing'))(response.headers)).tag, 'None');
  const missing = some(await app.get(base+'/missing')); assert.equal(missing.status,404); assert.equal(await app.is_success(missing),false);
  const binary = some(await app.get(base+'/binary')); assert.deepEqual(binary.body,[0,128,255]);
  const defaults = await app.defaults(base+'/echo');
  const echoed = some(await app.request({...defaults,method:'POST',body:{tag:'Bytes',value:[0,128,255]},headers:[{0:'x-request',1:'hello'}]}));
  assert.deepEqual(echoed.body,[0,128,255]); assert.equal(some(await (await app.header('x-request'))(echoed.headers)), 'hello');
  assert.equal(await app.text(some(await (await app.post(base+'/echo'))({tag:'Text',value:'héllo'}))), 'héllo');
  assert.equal(some(await app.get(base+'/redirect')).redirected,true);
  assert.equal(some(await app.request({...defaults,url:base+'/redirect',redirect:{tag:'Manual'}})).status,302);
  assert.equal((await app.request({...defaults,url:base+'/redirect',redirect:{tag:'Error'}})).value.kind.tag,'Network');
  for (const url of ['/slow','/slow-body']) assert.equal((await app.request({...defaults,url:base+url,timeout_ms:30})).value.kind.tag,'Timeout');
  assert.equal((await app.request({...defaults,url:base+'/big',max_bytes:100})).value.kind.tag,'TooLarge');
  assert.equal(some(await app.request({...defaults,url:base+'/big',max_bytes:1024})).body.length,1024);
  assert.equal(some(await app.request({...defaults,max_bytes:0})).body.length,0);
  for (const patch of [{url:'bad'}, {url:'file:///tmp/a'}, {method:'bad method'}, {body:{tag:'Text',value:'body'}}, {timeout_ms:0}, {timeout_ms:2147483648}, {headers:[{0:'bad\nheader',1:'value'}]}]) {
    assert.equal((await app.request({...defaults,...patch})).value.kind.tag,'InvalidRequest');
  }
  const originalFetch = globalThis.fetch;
  globalThis.fetch = () => Promise.reject(new Error('offline'));
  assert.equal((await app.get(base)).value.kind.tag,'Network');
  globalThis.fetch = undefined;
  assert.equal((await app.get(base)).value.kind.tag,'Unsupported');
  globalThis.fetch = originalFetch;
} finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
"#,
        );
    }
}

#[test]
fn local_handlers_replace_process_path_and_http_operations() {
    let project = project(
        r#"
let exercise: () -> _ = fn _ => handle do
  let args = std::process::args ()
  let env = std::process::env "name"
  let cwd = std::process::cwd ()
  let path = std::path::resolve ["virtual"]
  let relative = std::path::relative "a" "b"
  let response = std::http::post "https://invalid.example/" (#Text "body")
  return { args: args, env: env, cwd: cwd, path: path, relative: relative, response: response }
end with
| std::process::!Process.args _ => ["mock"]
| std::process::!Process.env name => #Some (#Some name)
| std::process::!Process.cwd _ => #Some "/virtual"
| std::path::!Path.resolve parts => #Some (std::str::join "/" parts)
| std::path::!Path.relative request => #Some request.to
| std::http::!Http.request request => #Some {
  status: 201n, status_text: request.method, headers: [], body: [65n8], url: request.url, redirected: false,
}
end
"#,
        "node",
        "library",
    );
    run(
        project.path(),
        r#"
const result = await app.exercise({});
assert.deepEqual(result.args,['mock']); assert.equal(result.env.value.value,'name');
assert.equal(result.cwd.value,'/virtual'); assert.equal(result.path.value,'virtual'); assert.equal(result.relative.value,'b');
assert.equal(result.response.value.status,201); assert.equal(result.response.value.status_text,'POST'); assert.deepEqual(result.response.value.body,[65]);
"#,
    );
}

#[test]
fn node_entry_handles_the_new_effects() {
    let project = project(
        r#"
let main = fn _ => do
  _ = std::process::args ()
  _ = std::process::env "RUDDY_API_TEST"
  _ = std::process::cwd ()
  _ = std::path::resolve []
  _ = std::path::relative "." ".."
  return match std::http::get "invalid" with
  | #Error { kind: #InvalidRequest, .. } => ()
  | _ => match std::process::exit 1n with end
  end
end
"#,
        "node",
        "executable",
    );
    run(project.path(), "");
}

#[test]
fn web_rejects_unhandled_process_effects_but_can_handle_them_locally() {
    let bad = project("let args = std::process::args", "web", "library");
    let error = ruddy_cli::check_project(bad.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("unsupported-export-effects"), "{error}");
    let good = project(
        r#"
let args: () -> _ = fn _ => handle std::process::args () with
| std::process::!Process.args _ => ["web"]
| std::process::!Process.env _ => #Some #None
| std::process::!Process.cwd _ => #Some "/"
end
"#,
        "web",
        "library",
    );
    run(
        good.path(),
        "assert.deepEqual(await app.args({}), ['web']);",
    );
}

#[test]
fn reflect_mirrors_describe_and_compare_types() {
    let project = project(
        r#"
type Dynamic = hide 'a => { value: 'a, evidence: Mirror 'a }
type Named = { name: String, count: Nat }
let described = std::reflect::describe (std::reflect::type_of { name: "a", count: 1n })
let field_names: std::reflect::Description -> [String] = fn description =>
  match std::array::get description.nodes description.root with
  | #Some (#Record fields) => std::array::map (fn field => field.name) fields
  | _ => []
  end
let names = field_names described
let items: [Dynamic] = [{ value: 2n, evidence: std::reflect::mirror () }, { value: "two", evidence: std::reflect::mirror () }]
let as_nat: Dynamic -> Option Nat = fn item => match item with
| hide 'x { value, evidence } => match std::reflect::same evidence (std::reflect::mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
let describe_later: Dynamic -> std::reflect::Description = fn item => match item with
| hide 'x { value, evidence } => (fn _ => std::reflect::describe (std::reflect::type_of value)) ()
end
let numbers = std::array::map as_nat items
let later = std::array::map describe_later items
let decode_mirror: ForeignValue -> std::result::Result (Mirror Nat) std::ffi::DecodeError = std::ffi::decode
let nat_mirror: Mirror Nat = std::reflect::mirror ()
"#,
        "node",
        "library",
    );
    run(
        project.path(),
        r#"
assert.deepEqual(app.names, ['count', 'name']);
assert.deepEqual(app.numbers, [{ tag: 'Some', value: 2 }, { tag: 'None', value: undefined }]);
assert.equal(app.later.length, 2);
assert.equal(app.later[0].nodes[0].tag, 'Nat');
assert.equal(app.later[1].nodes[0].tag, 'String');
assert.equal((await app.decode_mirror(app.nat_mirror)).tag, 'Some');
assert.equal((await app.decode_mirror({ nodes: ['Nat'] })).tag, 'Error');
"#,
    );
}

#[test]
fn reflect_shapes_print_and_rebuild_values() {
    let project = project(
        r##"
type Role = #Admin | #User Nat
type Person = { name: String, age: Nat, tags: [String], role: Role }
type Other = { name: String }
type Box = hide 'a => { value: 'a }
let concat = std::str::concat
@private
let join: [String] -> String = fn parts => match parts with
| [] => ""
| [only] => only
| [head, ..tail] => concat head (concat ", " (join tail))
end
@private
let print: Mirror 'a -> 'a -> String = fn mirror value => match std::reflect::shape mirror with
| #Nat { read, make } => std::str::from_nat (read value)
| #Int { read, make } => std::str::from_int (read value)
| #Real { read, make } => std::str::from_real (read value)
| #String { read, make } => concat "\"" (concat (read value) "\"")
| #Bool { read, make } => std::str::from_boolean (read value)
| #Array view => match view with
  | hide 'element { element, read, make } =>
    concat "[" (concat (join (std::array::map (print element) (read value))) "]")
  end
| #Record view => concat "{" (concat (join (std::array::map (fn field => match field with
    | hide 'field { name, mirror, read, bind, presence } => match read value with
      | #Some inner => concat name (concat ": " (print mirror inner))
      | #None => concat name ": absent"
      end
    end) view.fields)) "}")
| #Sum view => match std::array::filter_map (fn case => match case with
    | hide 'payload { name, mirror, project, inject } => match project value with
      | #Some payload => #Some (concat "#" (concat name (concat " " (print mirror payload))))
      | #None => #None
      end
    end) view.cases with
  | [text, ..] => text
  | [] => "?"
  end
| #Function description => "<function>"
| #Hidden description => "<hidden>"
| #Mirror description => "<mirror>"
| #Foreign => "<foreign>"
| _ => "<fixed>"
end
let person_mirror: Mirror Person = std::reflect::mirror ()
let role_mirror: Mirror Role = std::reflect::mirror ()
@private
let ada = { name: "Ada", age: 36n, tags: ["x", "y"], role: #User 3n }
let printed = print person_mirror ada
let printed_unit = print (std::reflect::type_of ()) ()
@private
let identity: Nat -> Nat = fn x => x
let printed_function = print (std::reflect::type_of identity) identity
let boxed: Box = { value: 1n }
let printed_hidden = print (std::reflect::type_of boxed) boxed
let printed_mirror = print (std::reflect::type_of person_mirror) person_mirror
let any = std::any::upcast 1n
let printed_any = print (std::reflect::type_of any) any
@private
extern host: ForeignValue = "1"
let printed_foreign = print (std::reflect::type_of host) host
@private
let bindings_of: Mirror 'a -> 'a -> [std::reflect::Binding 'a] = fn mirror value =>
  match std::reflect::shape mirror with
  | #Record view => std::array::filter_map (fn field => match field with
      | hide 'field { read, bind, .. } => match read value with
        | #Some inner => #Some (bind inner)
        | #None => #None
        end
      end) view.fields
  | _ => []
  end
@private
let build_with: Mirror 'a -> [std::reflect::Binding 'a] -> String = fn mirror bindings =>
  match std::reflect::shape mirror with
  | #Record view => match view.build bindings with
    | #Some rebuilt => concat "built " (print mirror rebuilt)
    | #Error (#Missing name) => concat "missing " name
    | #Error (#Duplicate name) => concat "duplicate " name
    | #Error (#Unknown name) => concat "unknown " name
    | #Error (#Foreign name) => concat "foreign " name
    | #Error (#Mismatched name) => concat "mismatched " name
    end
  | _ => "not a record"
  end
let all = bindings_of person_mirror ada
let rebuilt = build_with person_mirror all
let missing = build_with person_mirror (match all with | [_, ..rest] => rest | [] => [] end)
let duplicated = build_with person_mirror (std::array::concat all all)
let other_mirror: Mirror Other = std::reflect::mirror ()
extern reown: fn(std::reflect::Binding Person, Mirror Other) -> std::reflect::Binding Person = "(binding, record) => ({ ...binding, record })"
let first = match all with | [head, ..] => [head] | [] => [] end
let forged: () -> String = fn _ =>
  build_with person_mirror (std::array::map (fn binding => reown binding other_mirror) first)
@private
let make_admin: Mirror Role -> Option Role = fn mirror => match std::reflect::shape mirror with
| #Sum view => match std::array::filter_map (fn case => match case with
    | hide 'payload { name, mirror, inject, .. } =>
      match (name, inject, std::reflect::same (std::reflect::type_of ()) mirror) with
      | ("Admin", #Some make, #Some { forward, .. }) => #Some (make (forward ()))
      | _ => #None
      end
    end) view.cases with
  | [role, ..] => #Some role
  | [] => #None
  end
| _ => #None
end
let admin = match make_admin role_mirror with | #Some role => print role_mirror role | #None => "none" end
@private
let reversed: Mirror 'a -> 'a -> 'a = fn mirror value => match std::reflect::shape mirror with
| #Array view => match view with
  | hide 'element { read, make, .. } => make (std::array::reverse (read value))
  end
| _ => value
end
let tags_mirror: Mirror [String] = std::reflect::mirror ()
let backwards = reversed tags_mirror ada.tags
"##,
        "node",
        "library",
    );
    run(
        project.path(),
        r#"
assert.equal(app.printed, '{age: 36, name: "Ada", role: #User 3, tags: ["x", "y"]}');
assert.equal(app.printed_unit, '{}');
assert.equal(app.printed_function, '<function>');
assert.equal(app.printed_hidden, '<hidden>');
assert.equal(app.printed_mirror, '<mirror>');
assert.equal(app.printed_any, '<hidden>');
assert.equal(app.printed_foreign, '<foreign>');
assert.equal(app.rebuilt, 'built {age: 36, name: "Ada", role: #User 3, tags: ["x", "y"]}');
assert.equal(app.missing, 'missing age');
assert.equal(app.duplicated, 'duplicate age');
await assert.rejects(async () => app.forged({}), /package/);
assert.equal(app.admin, '#Admin {}');
assert.deepEqual(app.backwards, ['y', 'x']);
"#,
    );
}

#[test]
fn codecs_derive_json_round_trips_and_report_paths() {
    let project = project(
        r##"
type Role = #Admin | #User Nat
type Person = { name: String, age: Nat, tags: [String], role: Role, score: Real, small: Int8, wide: Int64 }
type Tree = #Leaf | #Node { left: Tree, value: Int, right: Tree }
let encode_person: Person -> Result String std::json::Error = std::json::encode
let decode_person: String -> Result Person std::json::Error = std::json::decode
let decode_people: String -> Result [Person] std::json::Error = std::json::decode
let encode_tree: Tree -> Result String std::json::Error = std::json::encode
let decode_tree: String -> Result Tree std::json::Error = std::json::decode
let decode_nat: String -> Result Nat std::json::Error = std::json::decode
let decode_int: String -> Result Int std::json::Error = std::json::decode
let decode_nat64: String -> Result Nat64 std::json::Error = std::json::decode
let decode_int64: String -> Result Int64 std::json::Error = std::json::decode
let decode_small: String -> Result Int8 std::json::Error = std::json::decode
let decode_real: String -> Result Real std::json::Error = std::json::decode
let encode_real: Real -> Result String std::json::Error = std::json::encode
let encode_function: (Nat -> Nat) -> Result String std::json::Error = std::json::encode
let decode_any: String -> Result Any std::json::Error = std::json::decode
let encode_nats: [Nat] -> Result String std::json::Error = std::json::encode
let parse = std::json::parse
let stringify = std::json::stringify
let to_real = std::json::number_to_real
let to_nat = std::json::number_to_nat
let to_int = std::json::number_to_int
let limited: String -> Result [[Nat]] std::json::Error = fn text =>
  match std::codec::derive_decoder (std::reflect::mirror ()) with
  | #Some decoder => std::json::decode_with decoder { depth: 2n, bytes: 40n, members: 3n, digits: 4n } text
  | #Error error => #Error (#Derive error)
  end
let shallow: String -> Result [[Nat]] std::json::Error = fn text =>
  match std::codec::derive_decoder (std::reflect::mirror ()) with
  | #Some decoder => std::json::decode_with decoder { depth: 1n, bytes: 40n, members: 3n, digits: 4n } text
  | #Error error => #Error (#Derive error)
  end
@private
let nat_schema = std::reflect::describe (std::reflect::type_of 0n)
@private
let as_text: std::codec::Codec Nat = {
  encoder: { schema: nat_schema, run: fn n => std::codec::!Write.write_text (std::str::from_nat n) },
  decoder: {
    schema: nat_schema,
    run: fn _ => std::result::and_then
      (fn text => match std::json::number_to_nat { token: text } with
        | #Some n => #Some n
        | #Error _ => std::codec::fail [] (#Custom "not a number")
        end)
      (std::codec::!Read.read_text ()),
  },
}
let encode_as_text: Nat -> Result String std::json::Error = std::json::encode_with as_text.encoder std::json::default_limits
let decode_as_text: String -> Result Nat std::json::Error = std::json::decode_with as_text.decoder std::json::default_limits
@private
let misused: std::codec::Encoder Nat = {
  schema: nat_schema,
  run: fn n => std::codec::!Write.end_record { frame: 7n },
}
@private
let silent: std::codec::Encoder Nat = { schema: nat_schema, run: fn n => #Some () }
@private
let twice: std::codec::Encoder Nat = {
  schema: nat_schema,
  run: fn n => std::result::and_then (fn _ => std::codec::!Write.write_nat n) (std::codec::!Write.write_nat n),
}
let encode_misused = std::json::encode_with misused std::json::default_limits
let encode_silent = std::json::encode_with silent std::json::default_limits
let encode_twice = std::json::encode_with twice std::json::default_limits
"##,
        "node",
        "library",
    );
    run(
        project.path(),
        r#"
const sum = (tag, value) => ({tag, value});
const big = (k, v) => typeof v === 'bigint' ? v.toString() + 'n' : v;
const norm = v => JSON.parse(JSON.stringify(v, big));
const some = result => { assert.equal(result.tag, 'Some', JSON.stringify(result, big)); return result.value; };
const error = result => { assert.equal(result.tag, 'Error', JSON.stringify(result, big)); return norm(result.value); };
const codec = result => { const e = error(result); assert.equal(e.tag, 'Codec'); return e.value; };
const ada = { name: 'Ada', age: 36, tags: ['x', 'y'], role: sum('User', 3), score: 0.1, small: -3, wide: 9223372036854775807n };
const text = some(await app.encode_person(ada));
assert.equal(text, '{"age":36,"name":"Ada","role":{"tag":"User","value":3},"score":0.1,"small":-3,"tags":["x","y"],"wide":9223372036854775807}');
assert.deepEqual(norm(some(await app.decode_person(text))), norm(ada));
assert.deepEqual(codec(await app.decode_person('{"name":"A","age":1,"tags":[],"role":{"tag":"Admin","value":{}},"score":1,"small":0,"wide":0,"extra":2}')), { path: [], kind: sum('Unknown', 'extra') });
assert.deepEqual(codec(await app.decode_person('{"name":"A"}')).kind, sum('Missing', 'age'));
assert.deepEqual(codec(await app.decode_person('{"name":"A","name":"B","age":1,"tags":[],"role":{"tag":"Admin","value":{}},"score":1,"small":0,"wide":0}')).kind, sum('Duplicate', 'name'));
const nested = codec(await app.decode_people('[' + text + ',{"name":"B","age":1,"tags":["ok",5],"role":{"tag":"Admin","value":{}},"score":1,"small":0,"wide":0}]'));
assert.deepEqual(nested.path, [sum('Index', 1), sum('Field', 'tags'), sum('Index', 1)]);
assert.deepEqual(nested.kind, sum('Unexpected', { expected: 'a string', found: 'a number' }));
const badCase = codec(await app.decode_person(text.replace('"User"', '"Guest"')));
assert.deepEqual(badCase, { path: [sum('Field', 'role')], kind: sum('UnexpectedCase', 'Guest') });
assert.deepEqual(codec(await app.decode_person(text.replace('"small":-3', '"small":300'))), { path: [sum('Field', 'small')], kind: sum('Range', { expected: 'Int8', found: '300' }) });
assert.deepEqual(codec(await app.decode_nat('12 x')).kind, sum('Parse', { message: 'unexpected trailing input', offset: 3 }));
assert.deepEqual(codec(await app.decode_nat('')).kind, sum('Unexpected', { expected: 'a number', found: 'the end of the input' }));
assert.equal(some(await app.decode_nat('1e3')), 1000);
assert.equal(some(await app.decode_nat('100e-2')), 1);
assert.equal(some(await app.decode_nat('-0')), 0);
assert.equal(some(await app.decode_int('-1.0')), -1);
assert.equal(codec(await app.decode_nat('1.5')).kind.tag, 'Range');
assert.equal(codec(await app.decode_nat('-1')).kind.tag, 'Range');
assert.equal(codec(await app.decode_nat('9007199254740992')).kind.tag, 'Range');
assert.equal(some(await app.decode_nat('9007199254740991')), 9007199254740991);
assert.equal(some(await app.decode_nat64('18446744073709551615')), 18446744073709551615n);
assert.equal(codec(await app.decode_nat64('18446744073709551616')).kind.tag, 'Range');
assert.equal(some(await app.decode_int64('-9223372036854775808')), -9223372036854775808n);
assert.equal(codec(await app.decode_int64('9223372036854775808')).kind.tag, 'Range');
assert.equal(some(await app.decode_small('-128')), -128);
assert.equal(codec(await app.decode_small('128')).kind.tag, 'Range');
assert.equal(some(await app.decode_real('0.1')), 0.1);
assert.ok(Object.is(some(await app.decode_real('-0')), -0));
assert.equal(codec(await app.decode_real('1e400')).kind.tag, 'Range');
assert.equal(codec(await app.decode_real('1e-400')).kind.tag, 'Range');
assert.equal(some(await app.decode_real('0e5')), 0);
assert.equal(some(await app.encode_real(-0)), '-0.0');
assert.equal(some(await app.encode_real(1e21)), '1e+21');
assert.equal(codec(await app.encode_real(NaN)).kind.tag, 'Range');
const tree = sum('Node', { left: sum('Leaf', {}), value: -5, right: sum('Node', { left: sum('Leaf', {}), value: 7, right: sum('Leaf', {}) }) });
const treeText = some(await app.encode_tree(tree));
assert.deepEqual(norm(some(await app.decode_tree(treeText))), norm(tree));
assert.deepEqual(error(await app.encode_function(x => x)), sum('Derive', { path: [], reason: 'a function has no default codec' }));
assert.deepEqual(error(await app.decode_any('1')).value.path, [sum('Field', 'value')].slice(0, 0));
assert.equal(error(await app.decode_any('1')).tag, 'Derive');
assert.equal(some(await app.encode_nats([1, 2, 3])), '[1,2,3]');
const doc = some(await app.parse('{"a":1,"a":2.50,"b":[true,null,"s\\u0041\\n"]}'));
assert.deepEqual(norm(doc), sum('Object', [
  {0: 'a', 1: sum('Number', { token: '1' })},
  {0: 'a', 1: sum('Number', { token: '2.50' })},
  {0: 'b', 1: sum('Array', [sum('Bool', true), { tag: 'Null' }, sum('String', 'sA\n')])},
]));
assert.equal(some(await app.stringify(doc)), '{"a":1,"a":2.50,"b":[true,null,"sA\\n"]}');
assert.equal(error(await app.stringify(sum('Number', { token: '01' }))).tag, 'Codec');
assert.equal(some(await app.to_real({ token: '2.50' })), 2.5);
assert.equal(some(await app.to_nat({ token: '2.50e2' })), 250);
assert.equal(some(await app.to_int({ token: '-7' })), -7);
assert.equal(error(await app.to_nat({ token: '2.5' })).tag, 'Codec');
assert.equal(codec(await app.parse('[1, 2')).kind.tag, 'Parse');
assert.equal(codec(await app.parse('{"a" 1}')).kind.tag, 'Parse');
assert.equal(codec(await app.parse('"\\x"')).kind.tag, 'Parse');
assert.equal(codec(await app.parse('"a\\u12"')).kind.tag, 'Parse');
assert.equal(codec(await app.parse('"a\nb"')).kind.tag, 'Parse');
assert.equal(codec(await app.parse('tru')).kind.tag, 'Parse');
assert.equal(codec(await app.parse('01')).kind.tag, 'Parse');
assert.equal(codec(await app.parse('1.')).kind.tag, 'Parse');
assert.equal(codec(await app.parse('1e')).kind.tag, 'Parse');
assert.equal(codec(await app.parse('[1,]')).kind.tag, 'Parse');
assert.equal(some(await app.limited('[[1],[2]]')).length, 2);
assert.deepEqual(codec(await app.limited('[[[1]]]')).kind, sum('Unexpected', { expected: 'a number', found: 'an array' }));
assert.deepEqual(codec(await app.shallow('[[1]]')), { path: [sum('Index', 0)], kind: sum('Limit', 'a document nested too deeply') });
assert.deepEqual(codec(await app.limited('[[1,2,3,4]]')).kind, sum('Limit', 'too many members'));
assert.deepEqual(codec(await app.limited('[[12345]]')).kind, sum('Limit', 'a number with too many digits'));
assert.deepEqual(codec(await app.limited('[[1],[1],[1],[1],[1],[1],[1],[1],[1],[1],[1],[1],[1]]')).kind, sum('Limit', 'a document longer than the limit'));
assert.equal(some(await app.encode_as_text(42)), '"42"');
assert.equal(some(await app.decode_as_text('"42"')), 42);
assert.deepEqual(codec(await app.decode_as_text('"x"')).kind, sum('Custom', 'not a number'));
assert.deepEqual(codec(await app.encode_misused(1)).kind, sum('Protocol', 'no frame is open'));
assert.deepEqual(codec(await app.encode_silent(1)).kind, sum('Protocol', 'the encoder did not write a whole document'));
assert.deepEqual(codec(await app.encode_twice(1)).kind, sum('Protocol', 'no value is expected here'));
"#,
    );
}

#[test]
fn strings_count_scalars_and_integers_have_one_zero() {
    let project = project(
        r#"
let len = std::str::len
let char_at: String -> Nat -> String = fn text index => std::str::char_at text index
let slice: String -> Nat -> Nat -> String = fn text start stop => std::str::slice text start stop
let index_of: String -> String -> Int = fn text search => std::str::index_of text search
let reverse = std::str::reverse
let pad_start: String -> Nat -> String -> String = fn text width fill => std::str::pad_start text width fill
let pad_end: String -> Nat -> String -> String = fn text width fill => std::str::pad_end text width fill
let less_than: String -> String -> Bool = fn left right => std::str::less_than left right
let chars = std::str::chars
let split: String -> String -> [String] = fn text separator => std::str::split text separator
let times: Int -> Int -> Int = fn left right => std::int::multiply left right
let negate = std::int::negate
let quotient: Int -> Int -> Int = fn left right => std::int::divide left right
let remainder: Int -> Int -> Int = fn left right => std::int::remainder left right
let truncate = std::int::from_real
let decode_text: String -> Result String std::json::Error = std::json::decode
let encode_text: String -> Result String std::json::Error = std::json::encode
"#,
        "node",
        "library",
    );
    run(
        project.path(),
        r#"
const some = result => { assert.equal(result.tag, 'Some', JSON.stringify(result)); return result.value; };
assert.equal(await app.len('😀é'), 2);
assert.equal(await (await app.char_at('a😀b'))(1), '😀');
assert.equal(await (await app.char_at('a😀b'))(3), '');
assert.equal(await (await (await app.slice('a😀bc'))(1))(3), '😀b');
assert.equal(await (await app.index_of('😀😀x'))('x'), 2);
assert.equal(await (await app.index_of('😀😀x'))('y'), -1);
assert.equal(await app.reverse('a😀b'), 'b😀a');
assert.equal(await (await (await app.pad_start('😀'))(3))('ab'), 'ab😀');
assert.equal(await (await (await app.pad_end('😀'))(4))('ab'), '😀aba');
assert.equal(await (await app.less_than('￿'))('😀'), true);
assert.equal(await (await app.less_than('😀'))('￿'), false);
assert.deepEqual(await app.chars('a😀'), ['a', '😀']);
assert.deepEqual(await (await app.split('😀,b'))(','), ['😀', 'b']);
for (const value of [await (await app.times(0))(-1), await app.negate(0), await (await app.quotient(0))(-5), await (await app.remainder(-4))(2), await app.truncate(-0.5)]) {
  assert.ok(Object.is(value, 0), String(value));
}
assert.equal(some(await app.decode_text('"\\ud83d\\ude00"')), '😀');
assert.equal((await app.decode_text('"\\ud83d"')).value.value.kind.tag, 'Parse');
assert.equal((await app.decode_text('"\\ude00"')).value.value.kind.tag, 'Parse');
assert.equal((await app.decode_text('"\\ud83dx"')).value.value.kind.tag, 'Parse');
assert.equal(some(await app.encode_text('😀\n')), '"😀\\n"');
"#,
    );
}

#[test]
fn binary_documents_registries_and_canonical_json_round_trip() {
    let project = project(
        r##"
type Role = #Admin | #User Nat
type Person = { name: String, age: Nat, tags: [String], role: Role, score: Real }
let encode_person: Person -> Result [Nat8] std::binary::Error = std::binary::encode
let decode_person: [Nat8] -> Result Person std::binary::Error = std::binary::decode
let encode_role: Role -> Result [Nat8] std::binary::Error = std::binary::encode
let decode_role: [Nat8] -> Result Role std::binary::Error = std::binary::decode
let encode_real: Real -> Result [Nat8] std::binary::Error = std::binary::encode
let decode_real: [Nat8] -> Result Real std::binary::Error = std::binary::decode
let decode_int: [Nat8] -> Result Int std::binary::Error = std::binary::decode
let encode_wide: Int64 -> Result [Nat8] std::binary::Error = std::binary::encode
let decode_wide: [Nat8] -> Result Int64 std::binary::Error = std::binary::decode
let decode_text: [Nat8] -> Result String std::binary::Error = std::binary::decode
let encode_function: (Nat -> Nat) -> Result [Nat8] std::binary::Error = std::binary::encode
@private
let person_mirror: Mirror Person = std::reflect::mirror ()
@private
let person_codec = std::codec::derive person_mirror
let schema = { id: "person", version: 2n }
let encode_versioned: Person -> Result [Nat8] std::binary::Error = fn person =>
  match person_codec with
  | #Some codec => std::binary::encode_versioned schema codec.encoder std::binary::default_limits person
  | #Error error => #Error (#Derive error)
  end
let decode_versioned: Nat -> [Nat8] -> Result Person std::binary::Error = fn version bytes =>
  match person_codec with
  | #Some codec => std::binary::decode_versioned { id: "person", version: version } codec.decoder std::binary::default_limits bytes
  | #Error error => #Error (#Derive error)
  end
let limited: [Nat8] -> Result [Nat] std::binary::Error = fn bytes =>
  match std::codec::derive_decoder (std::reflect::mirror ()) with
  | #Some decoder => std::binary::decode_with decoder { depth: 2n, bytes: 64n, members: 2n } bytes
  | #Error error => #Error (#Derive error)
  end
@private
let registry: std::codec::Registry = match std::codec::register "person" person_mirror with
| #Some entry => [entry]
| #Error _ => []
end
@private
let any_schema = std::reflect::describe (std::reflect::type_of ())
let encode_any: Any -> Result String std::json::Error = fn any =>
  std::json::encode_with { schema: any_schema, run: std::codec::encode_any registry } std::json::default_limits any
let decode_any: String -> Result Any std::json::Error = fn text =>
  std::json::decode_with { schema: any_schema, run: std::codec::decode_any registry } std::json::default_limits text
let ada: Person = { name: "Ada", age: 36n, tags: ["x"], role: #User 3n, score: 1.5 }
let boxed_ada = std::any::upcast ada
let boxed_nat = std::any::upcast 1n
let unbox_person: Any -> Option Person = std::any::downcast
let canonical_person: Person -> Result String std::json::Error = std::json::encode_canonical
let canon: String -> Result String std::json::Error = fn text =>
  std::result::and_then (fn document => std::json::stringify (std::json::canonical document)) (std::json::parse text)
"##,
        "node",
        "library",
    );
    run(
        project.path(),
        r#"
const big = (k, v) => typeof v === 'bigint' ? v.toString() + 'n' : v;
const norm = v => JSON.parse(JSON.stringify(v, big));
const some = result => { assert.equal(result.tag, 'Some', JSON.stringify(result, big)); return result.value; };
const codec = result => { assert.equal(result.tag, 'Error', JSON.stringify(result, big)); const e = norm(result.value); assert.equal(e.tag, 'Codec'); return e.value; };
const sum = (tag, value) => ({ tag, value });
const ada = { name: 'Ada', age: 36, tags: ['x'], role: sum('User', 3), score: 1.5 };
const bytes = some(await app.encode_person(ada));
const le64 = n => [n, 0, 0, 0, 0, 0, 0, 0];
assert.deepEqual(bytes, [...le64(36), 3, 0, 0, 0, 65, 100, 97, 1, 0, 0, 0, ...le64(3), 0, 0, 0, 0, 0, 0, 248, 63, 1, 0, 0, 0, 1, 0, 0, 0, 120]);
assert.deepEqual(norm(some(await app.decode_person(bytes))), norm(ada));
assert.deepEqual(codec(await app.decode_person(bytes.slice(0, 10))).kind, sum('Parse', { message: 'the input ends early', offset: 8 }));
assert.deepEqual(codec(await app.decode_person([...bytes, 0])).kind, sum('Parse', { message: 'unexpected trailing input', offset: bytes.length }));
assert.deepEqual(some(await app.encode_role(sum('Admin', {}))), [0, 0, 0, 0]);
assert.deepEqual(norm(some(await app.decode_role([1, 0, 0, 0, ...le64(9)]))), sum('User', 9));
assert.deepEqual(codec(await app.decode_role([2, 0, 0, 0])).kind.tag, 'Unexpected');
assert.ok(Object.is(some(await app.decode_real(some(await app.encode_real(-0)))), -0));
assert.ok(Number.isNaN(some(await app.decode_real(some(await app.encode_real(NaN))))));
assert.equal(some(await app.decode_int([255, 255, 255, 255, 255, 255, 255, 255])), -1);
assert.equal(codec(await app.decode_int([0, 0, 0, 0, 0, 0, 0, 128])).kind.tag, 'Range');
assert.equal(some(await app.decode_wide(some(await app.encode_wide(-9223372036854775808n)))), -9223372036854775808n);
assert.equal(some(await app.decode_text([4, 0, 0, 0, 240, 159, 152, 128])), '😀');
assert.equal(codec(await app.decode_text([2, 0, 0, 0, 255, 254])).kind.tag, 'Parse');
assert.equal(norm(await app.encode_function(x => x)).value.tag, 'Derive');
const versioned = some(await app.encode_versioned(ada));
assert.deepEqual(versioned.slice(0, 18), [6, 0, 0, 0, 112, 101, 114, 115, 111, 110, ...le64(2)]);
assert.deepEqual(norm(some(await (await app.decode_versioned(2))(versioned))), norm(ada));
assert.deepEqual(codec(await (await app.decode_versioned(3))(versioned)).kind, sum('Unexpected', { expected: 'schema person version 3', found: 'another schema' }));
assert.deepEqual(codec(await (await app.decode_versioned(2))(versioned.slice(0, 5))).kind.tag, 'Parse');
assert.deepEqual(some(await app.limited([2, 0, 0, 0, ...le64(1), ...le64(2)])), [1, 2]);
assert.deepEqual(codec(await app.limited([3, 0, 0, 0, ...le64(1), ...le64(2), ...le64(3)])).kind, sum('Limit', 'a sequence longer than the limit'));
const envelope = some(await app.encode_any(app.boxed_ada));
assert.equal(envelope, '{"id":"person","value":{"age":36,"name":"Ada","role":{"tag":"User","value":3},"score":1.5,"tags":["x"]}}');
const back = some(await app.decode_any(envelope));
assert.deepEqual(norm(some(await app.unbox_person(back))), norm(ada));
assert.deepEqual(codec(await app.encode_any(app.boxed_nat)).kind.tag, 'Unsupported');
assert.deepEqual(codec(await app.decode_any('{"id":"nobody","value":1}')).kind, sum('Unexpected', { expected: 'a registered identity', found: 'nobody' }));
assert.deepEqual(codec(await app.decode_any('{"value":1}')).kind, sum('Unknown', 'value'));
assert.equal(some(await app.canonical_person(ada)), '{"age":36,"name":"Ada","role":{"tag":"User","value":3},"score":1.5,"tags":["x"]}');
assert.equal(some(await app.canon('{"b": {"y":1,"x":[{"q":1,"p":2}]}, "a":2, "a":1}')), '{"a":2,"a":1,"b":{"x":[{"p":2,"q":1}],"y":1}}');
"#,
    );
}

#[test]
fn target_domains_are_reported_by_mirrors_and_checked_at_the_boundary() {
    let project = project(
        r#"
type Sizes = { count: Nat, delta: Int, small: Nat8, wide: Int64 }
let described = std::reflect::describe (std::reflect::type_of { count: 1n, delta: 1i, small: 1n8, wide: 1i64 })
let echo_nat: Nat -> Nat = fn n => n
let echo_int: Int -> Int = fn i => i
"#,
        "node",
        "library",
    );
    run(
        project.path(),
        r#"
const nodes = app.described.nodes;
const field = name => JSON.parse(JSON.stringify(nodes[nodes[app.described.root].value.find(f => f.name === name).node]));
assert.deepEqual(field('count'), { tag: 'Nat', value: { bits: 53, signed: false, min: '0', max: '9007199254740991' } });
assert.deepEqual(field('delta'), { tag: 'Int', value: { bits: 53, signed: true, min: '-9007199254740991', max: '9007199254740991' } });
assert.deepEqual(field('small'), { tag: 'Fixed', value: { bits: 8, signed: false, min: '0', max: '255' } });
assert.deepEqual(field('wide'), { tag: 'Fixed', value: { bits: 64, signed: true, min: '-9223372036854775808', max: '9223372036854775807' } });
assert.equal(await app.echo_nat(9007199254740991), 9007199254740991);
await assert.rejects(async () => app.echo_nat(9007199254740992), /Nat/);
await assert.rejects(async () => app.echo_int(-9007199254740992), /Int/);
assert.ok(Object.is(await app.echo_nat(-0), 0));
assert.ok(Object.is(await app.echo_int(-0), 0));
"#,
    );
}

#[test]
fn the_standard_library_reads_integers_within_the_bound_domain() {
    let source = r#"
let decode_nat: String -> Result Nat std::json::Error = std::json::decode
let decode_int: String -> Result Int std::json::Error = std::json::decode
let decode_nat64: String -> Result Nat64 std::json::Error = std::json::decode
let read_nat: [Nat8] -> Result Nat std::binary::Error = std::binary::decode
let read_int: [Nat8] -> Result Int std::binary::Error = std::binary::decode
let widen_nat32: Nat32 -> Nat = fn value => std::nat::to_nat32 value
let widen_nat64: Nat64 -> Nat = fn value => std::nat::to_nat64 value
let widen_int32: Int32 -> Int = fn value => std::int::to_int32 value
let widen_int64: Int64 -> Int = fn value => std::int::to_int64 value
"#;
    let helpers = r#"
const big = (key, value) => typeof value === 'bigint' ? value.toString() + 'n' : value;
const some = result => { assert.equal(result.tag, 'Some', JSON.stringify(result, big)); return result.value; };
const range = result => {
  assert.equal(result.tag, 'Error', JSON.stringify(result, big));
  assert.equal(result.value.tag, 'Codec');
  assert.equal(result.value.value.kind.tag, 'Range', JSON.stringify(result.value.value.kind, big));
  return JSON.parse(JSON.stringify(result.value.value.kind.value, big));
};
const outside = 'a 64-bit value outside the target\'s domain';
const out_of_range = { name: 'RangeError', message: 'integer out of range' };
"#;
    let narrow = domain_project(source, 32);
    run(
        narrow.path(),
        &format!(
            "{helpers}{}",
            r#"
assert.equal(some(await app.decode_nat('4294967295')), 4294967295);
assert.deepEqual(range(await app.decode_nat('4294967296')), { expected: 'Nat', found: '4294967296' });
assert.equal(some(await app.decode_int('-2147483648')), -2147483648);
assert.deepEqual(range(await app.decode_int('-2147483649')), { expected: 'Int', found: '-2147483649' });
assert.deepEqual(range(await app.decode_int('2147483648')), { expected: 'Int', found: '2147483648' });
assert.equal(some(await app.decode_nat64('18446744073709551615')), 18446744073709551615n);
assert.deepEqual(range(await app.read_nat([0, 0, 0, 0, 1, 0, 0, 0])), { expected: 'Nat', found: outside });
assert.equal(some(await app.read_nat([255, 255, 255, 255, 0, 0, 0, 0])), 4294967295);
assert.deepEqual(range(await app.read_int([0, 0, 0, 128, 0, 0, 0, 0])), { expected: 'Int', found: outside });
assert.equal(some(await app.read_int([0, 0, 0, 128, 255, 255, 255, 255])), -2147483648);
assert.equal(await app.widen_nat32(4294967295), 4294967295);
assert.equal(await app.widen_int32(-2147483648), -2147483648);
assert.equal(await app.widen_int64(-2147483648n), -2147483648);
await assert.rejects(async () => app.widen_nat64(4294967296n), out_of_range);
await assert.rejects(async () => app.widen_int64(2147483648n), out_of_range);
"#
        ),
    );
    let wide = domain_project(source, 53);
    run(
        wide.path(),
        &format!(
            "{helpers}{}",
            r#"
assert.equal(some(await app.decode_nat('4294967296')), 4294967296);
assert.equal(some(await app.decode_nat('9007199254740991')), 9007199254740991);
assert.deepEqual(range(await app.decode_nat('9007199254740992')), { expected: 'Nat', found: '9007199254740992' });
assert.equal(some(await app.decode_int('-2147483649')), -2147483649);
assert.equal(some(await app.decode_int('-9007199254740991')), -9007199254740991);
assert.deepEqual(range(await app.decode_int('-9007199254740992')), { expected: 'Int', found: '-9007199254740992' });
assert.equal(some(await app.decode_nat64('18446744073709551615')), 18446744073709551615n);
assert.equal(some(await app.read_nat([0, 0, 0, 0, 1, 0, 0, 0])), 4294967296);
assert.equal(some(await app.read_int([0, 0, 0, 128, 0, 0, 0, 0])), 2147483648);
assert.equal(await app.widen_nat32(4294967295), 4294967295);
assert.equal(await app.widen_nat64(4294967296n), 4294967296);
await assert.rejects(async () => app.widen_nat64(9007199254740992n), out_of_range);
await assert.rejects(async () => app.widen_nat64(18446744073709551615n), out_of_range);
await assert.rejects(async () => app.widen_int64(-9007199254740992n), out_of_range);
"#
        ),
    );
}

#[test]
fn descriptions_list_a_function_effects() {
    let project = project(
        r#"
effect Tick = () -> ()
@private
let ticking: () -> Nat + !Tick = fn _ => do _ = !Tick () return 2n end
@private
let pure: () -> Nat = fn _ => 1n
let described = std::reflect::describe (std::reflect::type_of ticking)
let described_pure = std::reflect::describe (std::reflect::type_of pure)
let same_as_pure = match std::reflect::same (std::reflect::type_of ticking) (std::reflect::type_of pure) with
| #Some _ => true
| #None => false
end
"#,
        "node",
        "library",
    );
    run(
        project.path(),
        r#"
const root = JSON.parse(JSON.stringify(app.described.nodes[app.described.root]));
assert.equal(root.tag, 'Function');
assert.equal(root.value.effects.length, 1);
assert.ok(root.value.effects[0].name.startsWith('effect:'), root.value.effects[0].name);
const pureRoot = JSON.parse(JSON.stringify(app.described_pure.nodes[app.described_pure.root]));
assert.deepEqual(pureRoot.value.effects, []);
assert.equal(app.same_as_pure, false);
"#,
    );
}
