#!/usr/bin/env python3
"""Generate Tauri icon assets from app-icon.svg using Python standard library."""
from pathlib import Path
import struct, zlib
APP=Path(__file__).resolve().parents[1]

def png(n):
 raw=bytearray()
 for y in range(n):
  raw.append(0)
  for x in range(n):
   X=x*1024/n;Y=y*1024/n;c=(20,22,28,255)
   if (X-512)**2+(Y-512)**2<330**2:c=(35,40,56,255)
   if 250<X<770 and 330<Y<740 and (X-510)**2/260**2+(Y-535)**2/205**2<1:c=(233,236,245,255)
   if (X-650)**2+(Y-445)**2<22**2:c=(20,22,28,255)
   raw.extend(c)
 def chunk(t,d):return struct.pack('>I',len(d))+t+d+struct.pack('>I',zlib.crc32(t+d)&0xffffffff)
 return b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>2I5B',n,n,8,6,0,0,0))+chunk(b'IDAT',zlib.compress(raw))+chunk(b'IEND',b'')
def main():
 p=APP/'src-tauri/icons';p.mkdir(parents=True,exist_ok=True)
 for n,name in [(32,'32x32.png'),(128,'128x128.png'),(256,'128x128@2x.png'),(1024,'icon.png')]: (p/name).write_bytes(png(n))
 chunks=[]
 for n,t in [(16,b'icp4'),(32,b'icp5'),(64,b'icp6'),(128,b'ic07'),(256,b'ic08'),(512,b'ic09'),(1024,b'ic10')]:
  b=png(n);chunks.append(t+struct.pack('>I',len(b)+8)+b)
 payload=b''.join(chunks);(p/'icon.icns').write_bytes(b'icns'+struct.pack('>I',len(payload)+8)+payload)
 b=png(32);(p/'icon.ico').write_bytes(b'\0\0\1\0\1\0\x01\x01\0\0\x01\0\x20\0'+struct.pack('<I',len(b))+struct.pack('<I',22)+b)
if __name__=='__main__':main()
