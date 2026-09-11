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
let decode: String -> std::result::Result { name: String, counts: [Nat] } std::json::DecodeError = std::json::decode
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
assert.equal((await app.parse('1e400')).tag, 'Error');
assert.equal((await app.stringify(sum('Number', Infinity))).tag, 'Error');
assert.equal((await app.stringify(sum('Number', NaN))).tag, 'Error');
assert.deepEqual({...some(await app.decode('{"name":"Ada","counts":[1,2,3]}'))}, {name:'Ada', counts:[1,2,3]});
const bad = await app.decode('{"name":"Ada","counts":[1,"bad"]}');
assert.equal(bad.tag, 'Error'); assert.equal(bad.value.tag, 'Decode'); assert.equal(bad.value.value.path, '$.counts[1]');
assert.equal((await app.decode('no')).value.tag, 'Parse');
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
let json: std::http::Response -> std::result::Result { answer: Nat } std::json::DecodeError = std::http::json
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
