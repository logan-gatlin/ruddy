"""Serve a local Git smart-HTTP fixture over TLS for the transport tests."""
import http.server
import os
from pathlib import Path
import ssl
import subprocess
import sys
import urllib.parse

root = Path(sys.argv[1])
cert, key = root / 'cert.pem', root / 'key.pem'
def openssl(*args):
    subprocess.run(['openssl', *args], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

openssl('req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', str(root / 'ca.key'),
        '-out', str(root / 'ca.pem'), '-days', '1', '-subj', '/CN=Ruddy test CA')
openssl('req', '-new', '-newkey', 'rsa:2048', '-nodes', '-keyout', str(key),
        '-out', str(root / 'server.csr'), '-subj', '/CN=127.0.0.1')
(root / 'extensions').write_text('subjectAltName=IP:127.0.0.1,DNS:localhost\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n')
openssl('x509', '-req', '-in', str(root / 'server.csr'), '-CA', str(root / 'ca.pem'),
        '-CAkey', str(root / 'ca.key'), '-CAcreateserial', '-out', str(cert), '-days', '1',
        '-extfile', str(root / 'extensions'))
backend = Path(subprocess.check_output(['git', '--exec-path'], text=True).strip()) / 'git-http-backend'

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        self.serve()

    def do_POST(self):
        self.serve()

    def serve(self):
        url = urllib.parse.urlsplit(self.path)
        if self.headers.get('Transfer-Encoding', '').lower() == 'chunked':
            parts = []
            while True:
                size = int(self.rfile.readline().split(b';')[0], 16)
                if not size:
                    self.rfile.readline()
                    break
                parts.append(self.rfile.read(size))
                self.rfile.read(2)
            body = b''.join(parts)
        else:
            body = self.rfile.read(int(self.headers.get('Content-Length', '0')))
        env = dict(os.environ, GIT_PROJECT_ROOT=str(root), GIT_HTTP_EXPORT_ALL='1',
                   REQUEST_METHOD=self.command, PATH_INFO=url.path, QUERY_STRING=url.query,
                   CONTENT_TYPE=self.headers.get('Content-Type', ''), CONTENT_LENGTH=str(len(body)),
                   GIT_PROTOCOL=os.environ.get('RUDDY_TEST_GIT_PROTOCOL', self.headers.get('Git-Protocol', '')))
        output = subprocess.run([str(backend)], input=body, env=env, capture_output=True, check=True).stdout
        headers, body = output.split(b'\r\n\r\n', 1)
        headers = [line.decode().split(': ', 1) for line in headers.split(b'\r\n')]
        status = next((int(value.split()[0]) for name, value in headers if name.lower() == 'status'), 200)
        self.send_response(status)
        self.send_header('Content-Length', str(len(body)))
        for name, value in headers:
            if name.lower() != 'status':
                self.send_header(name, value)
        self.end_headers()
        self.wfile.write(body)

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(cert, key)
server.socket = context.wrap_socket(server.socket, server_side=True)
print(server.server_address[1], flush=True)
server.serve_forever()
