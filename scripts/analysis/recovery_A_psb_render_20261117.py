"""Explicit diagnostic CPU render of locally owned PSB data.

Not a game/runtime test: no EXE, DLL, JS or native engine is executed.
Exports only a bounded preview and metadata under the dated archive.
Unknown masks/controllers/easing are reported; preview != compatibility.
"""
from __future__ import annotations

import argparse
from collections import Counter
from datetime import datetime, timezone
import json
import math
from pathlib import Path

import numpy as np
from PIL import Image

from recovery_A_20261116 import ROOT, sha
from recovery_A_psb_20261117 import Psb, load_sample


def content_at(layer, time):
    frames=layer["frameList"]
    before=[f for f in frames if f["time"]<=time]
    if not before or before[-1].get("type")==0:return None
    a=before[-1]
    result=dict(a.get("content",{}))
    after=[f for f in frames if f["time"]>time]
    if a.get("type")==3 and after and "content"in after[0]:
        b=after[0]
        ratio=(time-a["time"])/(b["time"]-a["time"])
        for key,default in [("coord",0),("scale",1)]:
            if key in result or key in b["content"]:
                av=result.get(key,[]);bv=b["content"].get(key,[])
                n=max(len(av),len(bv))
                result[key]=[(av[i] if i<len(av) else default)*(1-ratio)
                             +(bv[i] if i<len(bv) else default)*ratio for i in range(n)]
        for key,default in [("angle",0),("opa",255)]:
            if key in result or key in b["content"]:
                result[key]=result.get(key,default)*(1-ratio)+b["content"].get(key,default)*ratio
        am=result.get("mesh",{}).get("bp")
        bm=b["content"].get("mesh",{}).get("bp")
        if am and bm and len(am)==len(bm)==32:
            result["mesh"]={"bp":[x*(1-ratio)+y*ratio for x,y in zip(am,bm)],"cc":None}
    return result


class Renderer:
    def __init__(self, psb, width, height, scale):
        self.psb=psb;self.width=width;self.height=height;self.scale=scale
        self.pixels=np.zeros((height,width,4),dtype=np.float64)
        self.warnings=Counter();self.counts=Counter();self.vars={}
        self.textures={}
        for name,src in psb.root["source"].items():
            tex=src.get("texture")
            if tex is None:continue
            if tex["type"]!="RGBA8":raise ValueError("texture format")
            data=psb.chunk(tex["pixel"])
            assert len(data)==tex["width"]*tex["height"]*4
            array=np.frombuffer(data,dtype=np.uint8).reshape(tex["height"],tex["width"],4)
            self.textures[name]=array[:,:,[2,1,0,3]].astype(np.float64)
        self.active=set()

    def motion(self,obj,name,time,matrix=None,opacity=1,warps=()):
        key=(obj,name)
        if key in self.active:raise ValueError("motion cycle")
        self.active.add(key)
        motion=self.psb.root["object"][obj]["motion"][name]
        end=motion["lastTime"];loop=motion["loopTime"]
        if loop>=0 and end>loop and time>=end:time=loop+(time-loop)%(end-loop)
        previous_pending=getattr(self,"pending",None);self.pending=[]
        for layer in motion["layer"]:
            self.layer(layer,time,np.eye(3) if matrix is None else matrix,opacity,warps,motion["parameter"])
        pending=self.pending
        order=list(range(len(pending)))
        prior=[f for f in motion.get("priority",[]) if f["time"]<=time]
        if prior:order=prior[-1]["content"] or []
        for i in order:
            operation=pending[i]
            if operation is None:continue
            kind,content,matrix,opacity,warps,at=operation
            if kind==3 and "src"in content:
                self.motion(content["src"],content["icon"],at+content.get("motion",{}).get("timeOffset",0),
                            matrix,opacity,warps)
            elif kind==0 and content.get("src")not in [None,"blank"]:
                self.sprite(content,matrix,opacity,warps)
        self.pending=previous_pending
        self.active.remove(key)

    def layer(self,layer,time,matrix,opacity,warps,params):
        index=len(self.pending);self.pending.append(None)
        self.counts["layers"]+=1
        if self.counts["layers"]>16384:raise ValueError("render layer budget")
        if layer.get("stencilType") or layer["type"]==12:self.warnings["stencil/composite not applied"]+=1
        if layer["type"] not in [0,2,3,12]:
            self.warnings["unsupported layer type"]+=1;return
        if layer["parameterize"] is not None:
            p=params[layer["parameterize"]];value=self.vars.get(p["id"],0)
            time=max(0,min(1,(value-p["rangeBegin"])/(p["rangeEnd"]-p["rangeBegin"])))*p["division"]
        content=content_at(layer,time)
        if content is None:
            def hide(l):
                self.pending.append(None)
                for c in l["children"]:hide(c)
            for c in layer["children"]:hide(c)
            return
        previous=[f for f in layer["frameList"] if f["time"]<=time][-1]
        if previous["type"]==3 and previous["time"]!=time:self.warnings["linear easing not oracle-verified"]+=1
        x,y,*rest=content.get("coord",[0,0])
        if rest and rest[0]!=0:self.warnings["3D depth/perspective not applied"]+=1
        sx,sy=content.get("scale",[1,1])[:2]
        angle=math.radians(content.get("angle",0));c=math.cos(angle);s=math.sin(angle)
        local=np.array([[c*sx,-s*sy,x],[s*sx,c*sy,y],[0,0,1]],dtype=np.float64)
        matrix=matrix@local
        opacity*=content.get("opa",255)/255
        if content.get("color") is not None:self.warnings["color multiplier not applied"]+=1
        newwarps=warps
        mesh=content.get("mesh",{})
        if mesh.get("cc"):self.warnings["cc coefficients not applied"]+=1
        points=mesh.get("bp")
        if points:
            if content.get("src")=="blank" and len(points)==32:
                dims=list(map(float,content["icon"].split(":")))
                newwarps=warps+((matrix.copy(),dims,np.array(points).reshape(4,4,2)),)
                self.warnings["Bezier bp interpolation not oracle-verified"]+=1
            else:self.warnings["nonblank mesh/control point layout unsupported"]+=1
        self.pending[index]=(layer["type"],content,matrix,opacity,warps,time)
        for child in layer["children"]:self.layer(child,time,matrix,opacity,newwarps,params)

    @staticmethod
    def warp(point,warp):
        matrix,dims,points=warp
        local=np.linalg.solve(matrix,np.r_[point,1])
        width,height,ox,oy=dims
        u=np.clip((local[0]+ox)/width,0,1);v=np.clip((local[1]+oy)/height,0,1)
        def weights(t):return np.array([(1-t)**3,3*t*(1-t)**2,3*t*t*(1-t),t**3])
        bp=np.einsum("i,j,ijc->c",weights(v),weights(u),points)
        return (matrix@np.array([bp[0]*width-ox,bp[1]*height-oy,1]))[:2]

    def sprite(self,content,matrix,opacity,warps):
        source=content["src"];name=content["icon"]
        icon=self.psb.root["source"][source]["icon"][name]
        if content.get("bm",0):self.warnings["non-normal blend ignored"]+=1
        n=16 if warps else 1
        for y in range(n):
            for x in range(n):
                vertices=[]
                for dx,dy in [(0,0),(1,0),(0,1),(1,1)]:
                    u=(x+dx)/n;v=(y+dy)/n
                    point=(matrix@np.array([u*icon["width"]-icon["originX"],
                                           v*icon["height"]-icon["originY"],1]))[:2]
                    for warp in reversed(warps):point=self.warp(point,warp)
                    vertices.append([point[0]*self.scale+self.width/2,
                                     point[1]*self.scale+self.height*.53,
                                     icon["left"]+u*icon["width"],icon["top"]+v*icon["height"]])
                v=np.array(vertices);self.triangle(v[[0,1,2]],self.textures[source],opacity)
                self.triangle(v[[2,1,3]],self.textures[source],opacity)
                self.counts["triangles"]+=2
        self.counts["sprites"]+=1

    def triangle(self,v,texture,opacity):
        def edge(a,b,x,y):return (b[0]-a[0])*(y-a[1])-(b[1]-a[1])*(x-a[0])
        area=edge(v[0],v[1],v[2,0],v[2,1])
        if abs(area)<1e-12:return
        if area<0:v=v[[1,0,2]];area=-area
        lo=np.maximum(np.ceil(v[:,:2].min(axis=0)).astype(int),0)
        hi=np.minimum(np.ceil(v[:,:2].max(axis=0)).astype(int),[self.width,self.height])
        if any(hi<=lo):return
        yy,xx=np.mgrid[lo[1]:hi[1],lo[0]:hi[0]]
        edges=np.array([edge(v[1],v[2],xx,yy),edge(v[2],v[0],xx,yy),edge(v[0],v[1],xx,yy)])
        top=lambda a,b:b[1]<a[1]or(b[1]==a[1]and b[0]>a[0])
        inc=[top(v[1],v[2]),top(v[2],v[0]),top(v[0],v[1])]
        inside=np.all((edges>0)|((edges==0)&np.array(inc)[:,None,None]),axis=0)
        if not inside.any():return
        w=edges[:,inside]/area;uv=w.T@v[:,2:]
        tx=np.clip(uv[:,0]-.5,0,texture.shape[1]-1);ty=np.clip(uv[:,1]-.5,0,texture.shape[0]-1)
        x0=tx.astype(int);y0=ty.astype(int);x1=np.minimum(x0+1,texture.shape[1]-1);y1=np.minimum(y0+1,texture.shape[0]-1)
        fx=(tx-x0)[:,None];fy=(ty-y0)[:,None]
        color=(texture[y0,x0]*(1-fx)*(1-fy)+texture[y0,x1]*fx*(1-fy)
               +texture[y1,x0]*(1-fx)*fy+texture[y1,x1]*fx*fy)
        px=xx[inside];py=yy[inside];old=self.pixels[py,px];alpha=np.clip(color[:,3:]/255*opacity,0,1)
        result=np.c_[color[:,:3]*alpha+old[:,:3]*(1-alpha),(alpha+(old[:,3:]/255)*(1-alpha))*255]
        self.pixels[py,px]=np.round(np.clip(result,0,255))
        self.counts["sampled_pixels"]+=int(inside.sum())


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--index",type=int,default=0)
    parser.add_argument("--object")
    parser.add_argument("--motion")
    parser.add_argument("--variables",default="{}")
    parser.add_argument("--frame",type=float,default=0)
    parser.add_argument("--suffix",default="neutral")
    args=parser.parse_args()
    name,data=load_sample(args.index);psb=Psb(data)
    base=psb.root["metadata"]["base"]
    obj=args.object or base["chara"];motion=args.motion or base["motion"]
    renderer=Renderer(psb,512,768,.36);renderer.vars=json.loads(args.variables)
    renderer.motion(obj,motion,args.frame)
    pixels=renderer.pixels.astype(np.uint8)
    prefix=ROOT/f"archive/2026-11/20261117_psb_pixels_{args.index}_{args.suffix}"
    Image.fromarray(pixels).save(prefix.with_suffix(".png"))
    out={"generated_at_utc":datetime.now(timezone.utc).isoformat(),"resource":name,
         "object":obj,"motion":motion,"frame":args.frame,"variables":renderer.vars,
         "PSB_sha256":sha(data),"decrypted_header_key":psb.key,"value_nodes":psb.nodes,
         "pixel_sha256":sha(pixels.tobytes()),"nonzero_alpha_pixels":int((pixels[:,:,3]>0).sum()),
         "counts":renderer.counts,"warnings":renderer.warnings,"runtime_execution":False,
         "compatibility_claim":False,"preview_PNG":str(prefix.with_suffix(".png").relative_to(ROOT))}
    prefix.with_suffix(".json").write_text(json.dumps(out,ensure_ascii=False,indent=2)+"\n",encoding="utf-8")
    print(json.dumps({k:v for k,v in out.items() if k not in ["motion","object"]},ensure_ascii=True))


if __name__=="__main__":main()
