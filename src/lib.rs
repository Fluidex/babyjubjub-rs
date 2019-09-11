#[macro_use]
extern crate arrayref;
extern crate generic_array;
extern crate mimc_rs;
extern crate num;
extern crate num_bigint;
extern crate num_traits;
extern crate rand;

use blake2::{Blake2b, Digest};
use mimc_rs::Mimc7;
use poseidon_rs::Poseidon;
use std::cmp::min;

use num_bigint::{BigInt, RandBigInt, Sign, ToBigInt};
use num_traits::{One, Zero};

use generic_array::GenericArray;

mod utils;

#[macro_use]
extern crate lazy_static;

lazy_static! {
    static ref D: BigInt = BigInt::parse_bytes(b"168696", 10).unwrap();
    static ref A: BigInt = BigInt::parse_bytes(b"168700", 10).unwrap();
    static ref Q: BigInt = BigInt::parse_bytes(
        b"21888242871839275222246405745257275088548364400416034343698204186575808495617",
        10,
    )
    .unwrap();
    static ref B8: Point = Point {
        x: BigInt::parse_bytes(
            b"5299619240641551281634865583518297030282874472190772894086521144482721001553",
            10,
        )
        .unwrap(),
        y: BigInt::parse_bytes(
            b"16950150798460657717958625567821834550301663161624707787222815936182638968203",
            10,
        )
        .unwrap(),
    };
    static ref ORDER: BigInt = BigInt::parse_bytes(
        b"21888242871839275222246405745257275088614511777268538073601725287587578984328",
        10,
    )
    .unwrap();
    static ref SUBORDER: BigInt = &BigInt::parse_bytes(
        b"21888242871839275222246405745257275088614511777268538073601725287587578984328",
        10,
    )
    .unwrap()
        >> 3;
}

#[derive(Clone, Debug)]
pub struct Point {
    pub x: BigInt,
    pub y: BigInt,
}

impl Point {
    pub fn add(&self, q: &Point) -> Result<Point, String> {
        // x = (x1*y2+y1*x2)/(c*(1+d*x1*x2*y1*y2))
        // y = (y1*y2-x1*x2)/(c*(1-d*x1*x2*y1*y2))

        // x = (x1 * y2 + y1 * x2) / (1 + d * x1 * y1 * y2)
        let one: BigInt = One::one();
        let x_num: BigInt = &self.x * &q.y + &self.y * &q.x;
        let x_den: BigInt = &one + &D.clone() * &self.x * &q.x * &self.y * &q.y;
        let x_den_inv = utils::modinv(&x_den, &Q)?;
        let x: BigInt = utils::modulus(&(&x_num * &x_den_inv), &Q);

        // y = (y1 * y2 - a * x1 * x2) / (1 - d * x1 * x2 * y1 * y2)
        let y_num = &self.y * &q.y - &A.clone() * &self.x * &q.x;
        let y_den = utils::modulus(&(&one - &D.clone() * &self.x * &q.x * &self.y * &q.y), &Q);
        let y_den_inv = utils::modinv(&y_den, &Q)?;
        let y: BigInt = utils::modulus(&(&y_num * &y_den_inv), &Q);

        Ok(Point { x: x, y: y })
    }

    pub fn mul_scalar(&self, n: &BigInt) -> Result<Point, String> {
        let mut r: Point = Point {
            x: Zero::zero(),
            y: One::one(),
        };
        let mut rem: BigInt = n.clone();
        let mut exp: Point = self.clone();

        let zero: BigInt = Zero::zero();
        let one: BigInt = One::one();
        while rem != zero {
            let is_odd = &rem & &one == one;
            if is_odd == true {
                r = r.add(&exp)?;
            }
            exp = exp.add(&exp)?;
            rem = rem >> 1;
        }
        r.x = utils::modulus(&r.x, &Q);
        r.y = utils::modulus(&r.y, &Q);
        Ok(r)
    }

    pub fn compress(&self) -> [u8; 32] {
        let mut r: [u8; 32] = [0; 32];
        let (_, y_bytes) = self.y.to_bytes_le();
        let len = min(y_bytes.len(), r.len());
        r[..len].copy_from_slice(&y_bytes[..len]);
        if &self.x > &(&Q.clone() >> 1) {
            r[31] = r[31] | 0x80;
        }
        r
    }

    pub fn equals(&self, p: Point) -> bool {
        if self.x == p.x && self.y == p.y {
            return true;
        }
        false
    }
    pub fn zero() -> Point {
        return Point {
            x: Zero::zero(),
            y: Zero::zero(),
        };
    }
}

pub fn decompress_point(bb: [u8; 32]) -> Result<Point, String> {
    // https://tools.ietf.org/html/rfc8032#section-5.2.3
    let mut sign: bool = false;
    let mut b = bb.clone();
    if b[31] & 0x80 != 0x00 {
        sign = true;
        b[31] = b[31] & 0x7F;
    }
    let y: BigInt = BigInt::from_bytes_le(Sign::Plus, &b[..]);
    if y >= Q.clone() {
        return Err("y outside the Finite Field over R".to_string());
    }
    let one: BigInt = One::one();

    // x^2 = (1 - y^2) / (a - d * y^2) (mod p)
    let den = utils::modinv(
        &utils::modulus(
            &(&A.clone() - utils::modulus(&(&D.clone() * (&y * &y)), &Q)),
            &Q,
        ),
        &Q,
    )?;
    let mut x: BigInt = utils::modulus(&((one - utils::modulus(&(&y * &y), &Q)) * den), &Q);
    x = utils::modsqrt(&x, &Q)?;

    if sign && !(&x > &(&Q.clone() >> 1)) || (!sign && (&x > &(&Q.clone() >> 1))) {
        x = x * -1.to_bigint().unwrap();
    }
    x = utils::modulus(&x, &Q);
    Ok(Point { x: x, y: y })
}

pub struct Signature {
    r_b8: Point,
    s: BigInt,
}

impl Signature {
    pub fn compress(&self) -> [u8; 64] {
        let mut b: Vec<u8> = Vec::new();
        b.append(&mut self.r_b8.compress().to_vec());
        let (_, s_bytes) = self.s.to_bytes_le();
        let mut s_32bytes: [u8; 32] = [0; 32];
        let len = min(s_bytes.len(), s_32bytes.len());
        s_32bytes[..len].copy_from_slice(&s_bytes[..len]);
        b.append(&mut s_32bytes.to_vec());
        let mut r: [u8; 64] = [0; 64];
        r[..].copy_from_slice(&b[..]);
        r
    }
}

pub fn decompress_signature(b: &[u8; 64]) -> Result<Signature, String> {
    let r_b8_bytes: [u8; 32] = *array_ref!(b[..32], 0, 32);
    let s: BigInt = BigInt::from_bytes_le(Sign::Plus, &b[32..]);
    let r_b8 = decompress_point(r_b8_bytes);
    match r_b8 {
        Result::Err(err) => return Err(err.to_string()),
        Result::Ok(res) => Ok(Signature {
            r_b8: res.clone(),
            s: s,
        }),
    }
}

pub struct PrivateKey {
    key: BigInt,
}

impl PrivateKey {
    pub fn public(&self) -> Result<Point, String> {
        // https://tools.ietf.org/html/rfc8032#section-5.1.5
        let pk = B8.mul_scalar(&self.key)?;
        Ok(pk.clone())
    }

    pub fn sign_mimc(&self, msg: BigInt) -> Result<Signature, String> {
        // https://tools.ietf.org/html/rfc8032#section-5.1.6
        let mut hasher = Blake2b::new();
        let (_, sk_bytes) = self.key.to_bytes_be();
        hasher.input(sk_bytes);
        let mut h = hasher.result(); // h: hash(sk)
                                     // s: h[32:64]
        let s = GenericArray::<u8, generic_array::typenum::U32>::from_mut_slice(&mut h[32..64]);
        let (_, msg_bytes) = msg.to_bytes_be();
        let r_bytes = utils::concatenate_arrays(s, &msg_bytes);
        let mut r = BigInt::from_bytes_be(Sign::Plus, &r_bytes[..]);
        r = utils::modulus(&r, &SUBORDER);
        let r8: Point = B8.mul_scalar(&r)?;
        let a = &self.public()?;

        let hm_input = vec![r8.x.clone(), r8.y.clone(), a.x.clone(), a.y.clone(), msg];
        let mimc7 = Mimc7::new();
        let hm = mimc7.hash(hm_input)?;

        let mut s = &self.key << 3;
        s = hm * s;
        s = r + s;
        s = s % &SUBORDER.clone();

        Ok(Signature {
            r_b8: r8.clone(),
            s: s,
        })
    }
    pub fn sign_poseidon(&self, msg: BigInt) -> Result<Signature, String> {
        // https://tools.ietf.org/html/rfc8032#section-5.1.6
        let mut hasher = Blake2b::new();
        let (_, sk_bytes) = self.key.to_bytes_be();
        hasher.input(sk_bytes);
        let mut h = hasher.result(); // h: hash(sk)
                                     // s: h[32:64]
        let s = GenericArray::<u8, generic_array::typenum::U32>::from_mut_slice(&mut h[32..64]);
        let (_, msg_bytes) = msg.to_bytes_be();
        let r_bytes = utils::concatenate_arrays(s, &msg_bytes);
        let mut r = BigInt::from_bytes_be(Sign::Plus, &r_bytes[..]);
        r = utils::modulus(&r, &SUBORDER);
        let r8: Point = B8.mul_scalar(&r)?;
        let a = &self.public()?;

        let hm_input = vec![r8.x.clone(), r8.y.clone(), a.x.clone(), a.y.clone(), msg];
        let poseidon = Poseidon::new();
        let hm = poseidon.hash(hm_input)?;

        let mut s = &self.key << 3;
        s = hm * s;
        s = r + s;
        s = s % &SUBORDER.clone();

        Ok(Signature {
            r_b8: r8.clone(),
            s: s,
        })
    }

    pub fn sign_schnorr(&self, m: Vec<u8>) -> Result<(Point, BigInt), String> {
        // random r
        let mut rng = rand::thread_rng();
        let k = rng.gen_biguint(1024).to_bigint().unwrap();

        // r = k·G
        let r = B8.mul_scalar(&k)?;

        // h = H(x, r, m)
        let pk = &self.public()?;
        let h = hash_sig(&pk, m, &r)?;

        // s= k+x·h
        let s = k + &self.key * &h;
        Ok((r, s))
    }

    // https://eprint.iacr.org/2018/068.pdf
    pub fn sign_aggr(
        &self,
        x: Point,
        r: Point,
        ri: BigInt,
        l: Vec<u8>,
        m: Vec<u8>,
    ) -> Result<BigInt, String> {
        let pk = &self.public()?;
        // a0 = h(l, X0)
        let ai = hash_agg(&l, pk)?;

        // c = H(x, r, m)
        let c = hash_sig(&x, m, &r)?;
        // s0 = r0 + c · a0 · sk0 mod p
        let si = ri + c * ai * &self.key;
        Ok(utils::modulus(&si, &Q))
    }
}
pub fn calc_ri() -> Result<(BigInt, Point), String> {
    // random r
    let mut rng = rand::thread_rng();
    let k = rng.gen_biguint(1024).to_bigint().unwrap();

    // r = k·G
    let r = B8.mul_scalar(&k)?;
    Ok((k, r))
}
pub fn aggr_pks(mut pks: Vec<Point>, ris: Vec<Point>) -> Result<(Point, Point, Vec<u8>), String> {
    let mut pk_aggr: Vec<u8> = Vec::new();
    for i in 0..pks.len() {
        pk_aggr.append(&mut pks[i].compress().to_vec());
    }

    // (Xi=pk)
    // l = {X1, X2, X3, ...}
    let l = pk_aggr.clone();

    // X = sum{ h(l, Xi)·Xi } = sum{ Xi · ai }
    // a0 = h(l, X0)
    let ai = hash_agg(&l, &pks[0])?;
    let mut x: Point = pks[0].mul_scalar(&ai)?;
    for i in 1..pks.len() {
        // ai = h(l, Xi)
        let ai = hash_agg(&l, &pks[i])?;
        // x = x + ai · xi;
        let s = pks[i].mul_scalar(&ai)?;
        x = x.add(&s)?;
    }

    // r = sum{ ri }
    let mut r: Point = ris[0].clone();
    for i in 1..ris.len() {
        r = r.add(&ris[i])?;
    }

    Ok((x, r, l))
}
pub fn aggr_signatures(sigs: Vec<BigInt>) -> Result<BigInt, String> {
    let mut s: BigInt = Zero::zero();
    for i in 0..sigs.len() {
        // s = utils::modulus(&(s + &sigs[i]), &Q);
        s = s + &sigs[i];
    }
    Ok(utils::modulus(&s, &Q))
    // Ok(s)
}
pub fn verify_schnorr_aggregated(
    x: Point,
    r: Point,
    s: BigInt,
    m: Vec<u8>,
) -> Result<bool, String> {
    // sG = s·G
    let sg = B8.mul_scalar(&s)?;

    let c = hash_sig(&x, m, &r)?;
    let x_c = x.mul_scalar(&c)?;
    let right = r.add(&x_c)?;
    println!("x:\n{:?}\n{:?}\n", sg.x.to_string(), right.x.to_string());
    println!("y:\n{:?}\n{:?}\n", sg.y.to_string(), right.y.to_string());
    Ok(sg.equals(right))
}

pub fn hash_sig(pk: &Point, m: Vec<u8>, c: &Point) -> Result<BigInt, String> {
    let b: &mut Vec<u8> = &mut Vec::new();

    // other option could be to do it without compressing the points, and concatenating x|y
    b.append(&mut pk.compress().to_vec());
    b.append(&mut c.compress().to_vec());
    b.append(&mut m.clone());

    let poseidon = Poseidon::new();
    let h = poseidon.hash_bytes(b.to_vec())?;
    Ok(h)
}
pub fn hash_agg(l: &Vec<u8>, xi: &Point) -> Result<BigInt, String> {
    let b: &mut Vec<u8> = &mut Vec::new();

    // other option could be to do it without compressing the points, and concatenating x|y
    b.append(&mut l.clone());
    b.append(&mut xi.compress().to_vec());

    let poseidon = Poseidon::new();
    let h = poseidon.hash_bytes(b.to_vec())?;
    Ok(h)
}

pub fn verify_schnorr(pk: Point, m: Vec<u8>, r: Point, s: BigInt) -> Result<bool, String> {
    // sG = s·G
    let sg = B8.mul_scalar(&s)?;

    // r + h · x
    let h = hash_sig(&pk, m, &r)?;
    let pk_h = pk.mul_scalar(&h)?;
    let r_h_x = r.add(&pk_h)?;

    Ok(sg.equals(r_h_x))
}

pub fn new_key() -> PrivateKey {
    // https://tools.ietf.org/html/rfc8032#section-5.1.5
    let mut rng = rand::thread_rng();
    let sk_raw = rng.gen_biguint(1024).to_bigint().unwrap();

    let mut hasher = Blake2b::new();
    let (_, sk_raw_bytes) = sk_raw.to_bytes_be();
    hasher.input(sk_raw_bytes);
    let mut h = hasher.result();

    h[0] = h[0] & 0xF8;
    h[31] = h[31] & 0x7F;
    h[31] = h[31] | 0x40;

    let sk = BigInt::from_bytes_le(Sign::Plus, &h[..]);

    PrivateKey { key: sk }
}

pub fn verify_mimc(pk: Point, sig: Signature, msg: BigInt) -> bool {
    let hm_input = vec![
        sig.r_b8.x.clone(),
        sig.r_b8.y.clone(),
        pk.x.clone(),
        pk.y.clone(),
        msg,
    ];
    let mimc7 = Mimc7::new();
    let hm = match mimc7.hash(hm_input) {
        Result::Err(_) => return false,
        Result::Ok(hm) => hm,
    };
    let l = match B8.mul_scalar(&sig.s) {
        Result::Err(_) => return false,
        Result::Ok(l) => l,
    };
    let r = match sig
        .r_b8
        .add(&pk.mul_scalar(&(8.to_bigint().unwrap() * hm)).unwrap())
    {
        Result::Err(_) => return false,
        Result::Ok(r) => r,
    };
    l.equals(r)
}
pub fn verify_poseidon(pk: Point, sig: Signature, msg: BigInt) -> bool {
    let hm_input = vec![
        sig.r_b8.x.clone(),
        sig.r_b8.y.clone(),
        pk.x.clone(),
        pk.y.clone(),
        msg,
    ];
    let poseidon = Poseidon::new();
    let hm = match poseidon.hash(hm_input) {
        Result::Err(_) => return false,
        Result::Ok(hm) => hm,
    };
    let l = match B8.mul_scalar(&sig.s) {
        Result::Err(_) => return false,
        Result::Ok(l) => l,
    };
    let r = match sig
        .r_b8
        .add(&pk.mul_scalar(&(8.to_bigint().unwrap() * hm)).unwrap())
    {
        Result::Err(_) => return false,
        Result::Ok(r) => r,
    };
    l.equals(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate rustc_hex;
    use rustc_hex::{FromHex, ToHex};

    /*
    #[test]
    fn test_add_same_point() {
        let p: Point = Point {
            x: BigInt::parse_bytes(
                b"17777552123799933955779906779655732241715742912184938656739573121738514868268",
                10,
            )
            .unwrap(),
            y: BigInt::parse_bytes(
                b"2626589144620713026669568689430873010625803728049924121243784502389097019475",
                10,
            )
            .unwrap(),
        };
        let q: Point = Point {
            x: BigInt::parse_bytes(
                b"17777552123799933955779906779655732241715742912184938656739573121738514868268",
                10,
            )
            .unwrap(),
            y: BigInt::parse_bytes(
                b"2626589144620713026669568689430873010625803728049924121243784502389097019475",
                10,
            )
            .unwrap(),
        };
        let res = p.add(&q).unwrap();
        assert_eq!(
            res.x.to_string(),
            "6890855772600357754907169075114257697580319025794532037257385534741338397365"
        );
        assert_eq!(
            res.y.to_string(),
            "4338620300185947561074059802482547481416142213883829469920100239455078257889"
        );
    }
    #[test]
    fn test_add_different_points() {
        let p: Point = Point {
            x: BigInt::parse_bytes(
                b"17777552123799933955779906779655732241715742912184938656739573121738514868268",
                10,
            )
            .unwrap(),
            y: BigInt::parse_bytes(
                b"2626589144620713026669568689430873010625803728049924121243784502389097019475",
                10,
            )
            .unwrap(),
        };
        let q: Point = Point {
            x: BigInt::parse_bytes(
                b"16540640123574156134436876038791482806971768689494387082833631921987005038935",
                10,
            )
            .unwrap(),
            y: BigInt::parse_bytes(
                b"20819045374670962167435360035096875258406992893633759881276124905556507972311",
                10,
            )
            .unwrap(),
        };
        let res = p.add(&q).unwrap();
        assert_eq!(
            res.x.to_string(),
            "7916061937171219682591368294088513039687205273691143098332585753343424131937"
        );
        assert_eq!(
            res.y.to_string(),
            "14035240266687799601661095864649209771790948434046947201833777492504781204499"
        );
    }

    #[test]
    fn test_mul_scalar() {
        let p: Point = Point {
            x: BigInt::parse_bytes(
                b"17777552123799933955779906779655732241715742912184938656739573121738514868268",
                10,
            )
            .unwrap(),
            y: BigInt::parse_bytes(
                b"2626589144620713026669568689430873010625803728049924121243784502389097019475",
                10,
            )
            .unwrap(),
        };
        let res_m = p.mul_scalar(&3.to_bigint().unwrap()).unwrap();
        let res_a = p.add(&p).unwrap();
        let res_a = res_a.add(&p).unwrap();
        assert_eq!(res_m.x, res_a.x);
        assert_eq!(
            res_m.x.to_string(),
            "19372461775513343691590086534037741906533799473648040012278229434133483800898"
        );
        assert_eq!(
            res_m.y.to_string(),
            "9458658722007214007257525444427903161243386465067105737478306991484593958249"
        );

        let n = BigInt::parse_bytes(
            b"14035240266687799601661095864649209771790948434046947201833777492504781204499",
            10,
        )
        .unwrap();
        let res2 = p.mul_scalar(&n).unwrap();
        assert_eq!(
            res2.x.to_string(),
            "17070357974431721403481313912716834497662307308519659060910483826664480189605"
        );
        assert_eq!(
            res2.y.to_string(),
            "4014745322800118607127020275658861516666525056516280575712425373174125159339"
        );
    }

    #[test]
    fn test_new_key_sign_verify_mimc_0() {
        let sk = new_key();
        let pk = sk.public().unwrap();
        let msg = 5.to_bigint().unwrap();
        let sig = sk.sign_mimc(msg.clone()).unwrap();
        let v = verify_mimc(pk, sig, msg);
        assert_eq!(v, true);
    }

    #[test]
    fn test_new_key_sign_verify_mimc_1() {
        let sk = new_key();
        let pk = sk.public().unwrap();
        let msg = BigInt::parse_bytes(b"123456789012345678901234567890", 10).unwrap();
        let sig = sk.sign_mimc(msg.clone()).unwrap();
        let v = verify_mimc(pk, sig, msg);
        assert_eq!(v, true);
    }
    #[test]
    fn test_new_key_sign_verify_poseidon_0() {
        let sk = new_key();
        let pk = sk.public().unwrap();
        let msg = 5.to_bigint().unwrap();
        let sig = sk.sign_poseidon(msg.clone()).unwrap();
        let v = verify_poseidon(pk, sig, msg);
        assert_eq!(v, true);
    }

    #[test]
    fn test_new_key_sign_verify_poseidon_1() {
        let sk = new_key();
        let pk = sk.public().unwrap();
        let msg = BigInt::parse_bytes(b"123456789012345678901234567890", 10).unwrap();
        let sig = sk.sign_poseidon(msg.clone()).unwrap();
        let v = verify_poseidon(pk, sig, msg);
        assert_eq!(v, true);
    }

    #[test]
    fn test_point_compress_decompress() {
        let p: Point = Point {
            x: BigInt::parse_bytes(
                b"17777552123799933955779906779655732241715742912184938656739573121738514868268",
                10,
            )
            .unwrap(),
            y: BigInt::parse_bytes(
                b"2626589144620713026669568689430873010625803728049924121243784502389097019475",
                10,
            )
            .unwrap(),
        };
        let p_comp = p.compress();
        assert_eq!(
            p_comp[..].to_hex(),
            "53b81ed5bffe9545b54016234682e7b2f699bd42a5e9eae27ff4051bc698ce85"
        );
        let p2 = decompress_point(p_comp).unwrap();
        assert_eq!(p.x, p2.x);
        assert_eq!(p.y, p2.y);
    }

    #[test]
    fn test_point_decompress0() {
        let y_bytes_raw = "b5328f8791d48f20bec6e481d91c7ada235f1facf22547901c18656b6c3e042f"
            .from_hex()
            .unwrap();
        let mut y_bytes: [u8; 32] = [0; 32];
        y_bytes.copy_from_slice(&y_bytes_raw);
        let p = decompress_point(y_bytes).unwrap();

        let expected_px_raw = "b86cc8d9c97daef0afe1a4753c54fb2d8a530dc74c7eee4e72b3fdf2496d2113"
            .from_hex()
            .unwrap();
        let mut e_px_bytes: [u8; 32] = [0; 32];
        e_px_bytes.copy_from_slice(&expected_px_raw);
        let expected_px: BigInt = BigInt::from_bytes_le(Sign::Plus, &e_px_bytes);
        assert_eq!(&p.x, &expected_px);
    }

    #[test]
    fn test_point_decompress1() {
        let y_bytes_raw = "70552d3ff548e09266ded29b33ce75139672b062b02aa66bb0d9247ffecf1d0b"
            .from_hex()
            .unwrap();
        let mut y_bytes: [u8; 32] = [0; 32];
        y_bytes.copy_from_slice(&y_bytes_raw);
        let p = decompress_point(y_bytes).unwrap();

        let expected_px_raw = "30f1635ba7d56f9cb32c3ffbe6dca508a68c7f43936af11a23c785ce98cb3404"
            .from_hex()
            .unwrap();
        let mut e_px_bytes: [u8; 32] = [0; 32];
        e_px_bytes.copy_from_slice(&expected_px_raw);
        let expected_px: BigInt = BigInt::from_bytes_le(Sign::Plus, &e_px_bytes);
        assert_eq!(&p.x, &expected_px);
    }

    #[test]
    fn test_point_decompress_loop() {
        for _ in 0..5 {
            let mut rng = rand::thread_rng();
            let sk_raw = rng.gen_biguint(1024).to_bigint().unwrap();
            let mut hasher = Blake2b::new();
            let (_, sk_raw_bytes) = sk_raw.to_bytes_be();
            hasher.input(sk_raw_bytes);
            let mut h = hasher.result();

            h[0] = h[0] & 0xF8;
            h[31] = h[31] & 0x7F;
            h[31] = h[31] | 0x40;

            let sk = BigInt::from_bytes_le(Sign::Plus, &h[..]);
            let point = B8.mul_scalar(&sk).unwrap();
            let cmp_point = point.compress();
            let dcmp_point = decompress_point(cmp_point).unwrap();

            assert_eq!(&point.x, &dcmp_point.x);
            assert_eq!(&point.y, &dcmp_point.y);
        }
    }

    #[test]
    fn test_signature_compress_decompress() {
        let sk = new_key();
        let pk = sk.public().unwrap();

        for i in 0..5 {
            let msg_raw = "123456".to_owned() + &i.to_string();
            let msg = BigInt::parse_bytes(msg_raw.as_bytes(), 10).unwrap();
            let sig = sk.sign_mimc(msg.clone()).unwrap();

            let compressed_sig = sig.compress();
            let decompressed_sig = decompress_signature(&compressed_sig).unwrap();
            assert_eq!(&sig.r_b8.x, &decompressed_sig.r_b8.x);
            assert_eq!(&sig.r_b8.y, &decompressed_sig.r_b8.y);
            assert_eq!(&sig.s, &decompressed_sig.s);

            let v = verify_mimc(pk.clone(), decompressed_sig, msg);
            assert_eq!(v, true);
        }
    }

    #[test]
    fn test_schnorr_signature() {
        let sk = new_key();
        let pk = sk.public().unwrap();

        let msg: Vec<u8> = ("123456".to_owned() + &1.to_string()).as_bytes().to_vec();
        let (s, e) = sk.sign_schnorr(msg.clone()).unwrap();
        println!("s {:?}", s.x.to_string());
        println!("s {:?}", s.y.to_string());
        println!("e {:?}", e.to_string());
        let verification = verify_schnorr(pk, msg, s, e).unwrap();
        assert_eq!(true, verification);
    }
    */
    #[test]
    fn test_aggregated_schnorr_signature() {
        let m: Vec<u8> = ("123456".to_owned() + &1.to_string()).as_bytes().to_vec();

        let sk0 = new_key();
        let pk0 = sk0.public().unwrap();
        let sk1 = new_key();
        let pk1 = sk1.public().unwrap();
        let mut pks: Vec<Point> = Vec::new();
        pks.push(pk0.clone());
        pks.push(pk1.clone());

        let (k0, r0) = calc_ri().unwrap();
        let (k1, r1) = calc_ri().unwrap();
        let mut ris: Vec<Point> = Vec::new();
        ris.push(r0);
        ris.push(r1);

        // aggregate public keys
        let (x, r, l) = aggr_pks(pks, ris).unwrap();

        // perform the signs
        let sig0 = sk0
            .sign_aggr(x.clone(), r.clone(), k0, l.clone(), m.clone())
            .unwrap();
        let sig1 = sk1
            .sign_aggr(x.clone(), r.clone(), k1, l.clone(), m.clone())
            .unwrap();

        // aggregate the signatures
        let mut sigs: Vec<BigInt> = Vec::new();
        sigs.push(sig0);
        sigs.push(sig1);
        let aggr_sig = aggr_signatures(sigs).unwrap();

        // verify the pk with the aggregated signatures
        let verification = verify_schnorr_aggregated(x, r, aggr_sig, m).unwrap();
        assert_eq!(true, verification);
    }
}
