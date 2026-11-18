//! Small independent row-vector matrix core for the fixed PPI geometry
//! consumers. The recovered helper prepends a new matrix to the current one.
pub(super) type Matrix=[f32;16];
pub(super) fn identity()->Matrix {[1.,0.,0.,0.,0.,1.,0.,0.,0.,0.,1.,0.,0.,0.,0.,1.]}
pub(super) fn product(a:Matrix,b:Matrix)->Matrix {
    std::array::from_fn(|i| {
        let r=i/4;let c=i%4;
        ((a[r*4]*b[c]+a[r*4+1]*b[c+4])+a[r*4+2]*b[c+8])+a[r*4+3]*b[c+12]
    })
}
pub(super) fn transform(m:Matrix,p:[f32;3])->[f32;3] {
    std::array::from_fn(|i|((m[i]*p[0]+m[i+4]*p[1])+m[i+8]*p[2])+m[i+12])
}
pub(super) fn affine(size:[f32;3],angles:[f32;3],position:[f32;3])->Matrix {
    let mut m=identity();m[0]=size[0];m[5]=size[1];m[10]=size[2];
    for axis in 0..3 {
        //Each original rotation helper truncates radians*1000, converts
        //back to float and divides by1000 before its double sin/cos call.
        let angle=super::trunc(angles[axis]*1000.) as f32/1000.;
        let (s,c)=f64::from(angle).sin_cos();let(s,c)=(s as f32,c as f32);
        let mut r=identity();
        match axis {
            0=>{r[5]=c;r[6]=s;r[9]=-s;r[10]=c;},
            1=>{r[0]=c;r[2]=-s;r[8]=s;r[10]=c;},
            _=>{r[0]=c;r[1]=s;r[4]=-s;r[5]=c;},
        }
        m=product(r,m);
    }
    for i in 0..3 {m[12+i]+=position[i];}m
}
