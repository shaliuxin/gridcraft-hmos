// 镜像 hvigor DecipherUtil 的密钥推导 + AES-128-GCM 加密，
// 把明文口令转成 build-profile.json5 里 storePassword/keyPassword 要的混淆 hex 形式。
// 复用 signing/material/（本地可复用密钥，与 bundle 无关）。
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

const MATERIAL_DIR = path.resolve(__dirname, '..', 'signing', 'material');
const component = new Int8Array([49,243,9,115,214,175,91,184,211,190,177,88,101,131,192,119]);

function readDirBytes(dir){ const e = fs.readdirSync(dir).filter(x=>x!=='.DS_Store'); return new Int8Array(fs.readFileSync(path.resolve(dir, e[0]))); }
function xor(a,b){ const i=new Int8Array(a.byteLength); for(let j=0;j<a.byteLength;j++) i[j]=a[j]^b[j]; return i; }
function xorComponents(arr){ let e=xor(arr[0],arr[1]); for(let i=2;i<arr.length;i++) e=xor(e,arr[i]); return e; }
function readFd(fdDir){ return fs.readdirSync(fdDir).filter(t=>t!=='.DS_Store').map(e=>readDirBytes(path.resolve(fdDir,e))); }

function getKey(){
  const material = MATERIAL_DIR;
  const fd = readFd(path.resolve(material,'fd'));
  const salt = readDirBytes(path.resolve(material,'ac'));
  // getRootKey
  const i = fd.concat(component);
  const x = xorComponents(i);
  const rootKey = new Int8Array(crypto.pbkdf2Sync(Buffer.from(x).toString('utf-8'), Buffer.from(salt), 10000, 16, 'sha256'));
  // 用 rootKey 解出 work material(ce) 得到真正的 AES 密钥
  const work = readDirBytes(path.resolve(material,'ce'));
  const e = (work[0]&255)<<24 | (work[1]&255)<<16 | (work[2]&255)<<8 | (work[3]&255);
  const ivlen = work.length - 4 - e;
  const iv = Buffer.from(work.slice(4, 4+ivlen));
  const tag = Buffer.from(work.slice(work.length-16));
  const decipher = crypto.createDecipheriv('aes-128-gcm', Buffer.from(rootKey), iv);
  decipher.setAuthTag(tag);
  const ct = Buffer.from(work.slice(4+ivlen, work.length-16));
  return Buffer.concat([decipher.update(ct), decipher.final()]);
}

function encrypt(password){
  const key = getKey();
  const iv = crypto.randomBytes(12);
  const cipher = crypto.createCipheriv('aes-128-gcm', key, iv);
  const ct = Buffer.concat([cipher.update(Buffer.from(password,'utf-8')), cipher.final()]);
  const tag = cipher.getAuthTag();
  const N = Buffer.byteLength(password,'utf-8');
  const first4 = N + 16;           // 与 hvigor 解密约定一致（原样：N+16）
  const ivlen = iv.length;         // 12
  const total = 4 + ivlen + ct.length + 16;
  const out = Buffer.alloc(total);
  out.writeUInt32BE(first4, 0);
  iv.copy(out, 4);
  ct.copy(out, 4+ivlen);
  tag.copy(out, 4+ivlen+ct.length);
  return out.toString('hex');
}

const pwd = process.argv[2];
if(!pwd){ console.error('usage: node encrypt_pwd.js <password>'); process.exit(1); }
// 自检：解密回来应等于原文
const key = getKey();
process.stdout.write(encrypt(pwd) + '\n');
