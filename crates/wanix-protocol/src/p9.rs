//! Rust-native 9P2000.L subset used for Wanix import/export bridges.
//!
//! The module is intentionally frame-oriented: browser MessagePort, websocket,
//! and native stream adapters can all deliver complete 9P packets to the same
//! server/client bridge without depending on the reference Go implementation.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, SeekFrom, Write};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use wanix_core::{base_name, clean_path, parent_path, valid_path};
use wanix_fs::{
    self as fs, BoxFile, DirEntry, Error, ErrorKind, FileHandle, FileMode, FileSystem, FsContext,
    FsRef, Metadata, OpenFlags, Result,
};

pub const VERSION: &str = "9P2000.L";
pub const DEFAULT_MSIZE: u32 = 64 * 1024;
pub const NOFID: u32 = u32::MAX;
pub const NOTAG: u16 = u16::MAX;
pub const AT_REMOVEDIR: u32 = 0x200;
pub const MAX_STREAM_FRAME_SIZE: usize = DEFAULT_MSIZE as usize;

const QTDIR: u8 = 0x80;
const QTSYMLINK: u8 = 0x02;

const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;
const DT_LNK: u8 = 10;

const ATTR_MODE: u64 = 1 << 0;
const ATTR_NLINK: u64 = 1 << 1;
const ATTR_UID: u64 = 1 << 2;
const ATTR_GID: u64 = 1 << 3;
const ATTR_RDEV: u64 = 1 << 4;
const ATTR_ATIME: u64 = 1 << 5;
const ATTR_MTIME: u64 = 1 << 6;
const ATTR_CTIME: u64 = 1 << 7;
const ATTR_SIZE: u64 = 1 << 9;
const ATTR_BLOCKS: u64 = 1 << 10;
const ATTR_BASIC: u64 = ATTR_MODE
    | ATTR_NLINK
    | ATTR_UID
    | ATTR_GID
    | ATTR_RDEV
    | ATTR_ATIME
    | ATTR_MTIME
    | ATTR_CTIME
    | ATTR_SIZE
    | ATTR_BLOCKS;

pub const SETATTR_MODE: u32 = 1 << 0;
pub const SETATTR_UID: u32 = 1 << 1;
pub const SETATTR_GID: u32 = 1 << 2;
pub const SETATTR_SIZE: u32 = 1 << 3;
pub const SETATTR_ATIME: u32 = 1 << 4;
pub const SETATTR_MTIME: u32 = 1 << 5;
pub const SETATTR_ATIME_SET: u32 = 1 << 7;
pub const SETATTR_MTIME_SET: u32 = 1 << 8;

const EPERM: u32 = 1;
const ENOENT: u32 = 2;
const EIO: u32 = 5;
const EBADF: u32 = 9;
const EACCES: u32 = 13;
const EEXIST: u32 = 17;
const ENOTDIR: u32 = 20;
const EISDIR: u32 = 21;
const EINVAL: u32 = 22;
const ENOSYS: u32 = 38;
const ENOTEMPTY: u32 = 39;

mod msg {
    pub const RLERROR: u8 = 7;
    pub const TLOPEN: u8 = 12;
    pub const RLOPEN: u8 = 13;
    pub const TLCREATE: u8 = 14;
    pub const RLCREATE: u8 = 15;
    pub const TSYMLINK: u8 = 16;
    pub const RSYMLINK: u8 = 17;
    pub const TREADLINK: u8 = 22;
    pub const RREADLINK: u8 = 23;
    pub const TGETATTR: u8 = 24;
    pub const RGETATTR: u8 = 25;
    pub const TSETATTR: u8 = 26;
    pub const RSETATTR: u8 = 27;
    pub const TXATTRWALK: u8 = 30;
    pub const RXATTRWALK: u8 = 31;
    pub const TXATTRCREATE: u8 = 32;
    pub const RXATTRCREATE: u8 = 33;
    pub const TREADDIR: u8 = 40;
    pub const RREADDIR: u8 = 41;
    pub const TFSYNC: u8 = 50;
    pub const RFSYNC: u8 = 51;
    pub const TLINK: u8 = 70;
    pub const RLINK: u8 = 71;
    pub const TMKDIR: u8 = 72;
    pub const RMKDIR: u8 = 73;
    pub const TRENAMEAT: u8 = 74;
    pub const RRENAMEAT: u8 = 75;
    pub const TUNLINKAT: u8 = 76;
    pub const RUNLINKAT: u8 = 77;
    pub const TVERSION: u8 = 100;
    pub const RVERSION: u8 = 101;
    pub const TATTACH: u8 = 104;
    pub const RATTACH: u8 = 105;
    pub const TFLUSH: u8 = 108;
    pub const RFLUSH: u8 = 109;
    pub const TWALK: u8 = 110;
    pub const RWALK: u8 = 111;
    pub const TREAD: u8 = 116;
    pub const RREAD: u8 = 117;
    pub const TWRITE: u8 = 118;
    pub const RWRITE: u8 = 119;
    pub const TCLUNK: u8 = 120;
    pub const RCLUNK: u8 = 121;
    pub const TREMOVE: u8 = 122;
    pub const RREMOVE: u8 = 123;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Qid {
    pub typ: u8,
    pub version: u32,
    pub path: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NinePAttr {
    pub valid: u64,
    pub qid: Qid,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u64,
    pub rdev: u64,
    pub size: u64,
    pub blksize: u64,
    pub blocks: u64,
    pub atime_seconds: u64,
    pub atime_nanoseconds: u64,
    pub mtime_seconds: u64,
    pub mtime_nanoseconds: u64,
    pub ctime_seconds: u64,
    pub ctime_nanoseconds: u64,
    pub btime_seconds: u64,
    pub btime_nanoseconds: u64,
    pub gen: u64,
    pub data_version: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetAttr {
    pub valid: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub atime_seconds: u64,
    pub atime_nanoseconds: u64,
    pub mtime_seconds: u64,
    pub mtime_nanoseconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NinePDirent {
    pub qid: Qid,
    pub offset: u64,
    pub typ: u8,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NinePRequest {
    Version {
        msize: u32,
        version: String,
    },
    Attach {
        fid: u32,
        afid: u32,
        uname: String,
        aname: String,
        n_uname: u32,
    },
    Walk {
        fid: u32,
        newfid: u32,
        names: Vec<String>,
    },
    Lopen {
        fid: u32,
        flags: u32,
    },
    Lcreate {
        fid: u32,
        name: String,
        flags: u32,
        mode: u32,
        gid: u32,
    },
    GetAttr {
        fid: u32,
        request_mask: u64,
    },
    SetAttr {
        fid: u32,
        attr: SetAttr,
    },
    XattrWalk {
        fid: u32,
        newfid: u32,
        name: String,
    },
    XattrCreate {
        fid: u32,
        name: String,
        size: u64,
        flags: u32,
    },
    Read {
        fid: u32,
        offset: u64,
        count: u32,
    },
    Write {
        fid: u32,
        offset: u64,
        data: Vec<u8>,
    },
    Clunk {
        fid: u32,
    },
    Remove {
        fid: u32,
    },
    Mkdir {
        fid: u32,
        name: String,
        mode: u32,
        gid: u32,
    },
    Link {
        fid: u32,
        newdirfid: u32,
        name: String,
    },
    Readdir {
        fid: u32,
        offset: u64,
        count: u32,
    },
    RenameAt {
        olddirfid: u32,
        oldname: String,
        newdirfid: u32,
        newname: String,
    },
    UnlinkAt {
        dirfid: u32,
        name: String,
        flags: u32,
    },
    Symlink {
        fid: u32,
        name: String,
        target: String,
        gid: u32,
    },
    Readlink {
        fid: u32,
    },
    Fsync {
        fid: u32,
    },
    Flush {
        oldtag: u16,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NinePResponse {
    Lerror { ecode: u32 },
    Version { msize: u32, version: String },
    Attach { qid: Qid },
    Walk { qids: Vec<Qid> },
    Lopen { qid: Qid, iounit: u32 },
    Lcreate { qid: Qid, iounit: u32 },
    GetAttr { attr: NinePAttr },
    SetAttr,
    XattrWalk { size: u64 },
    XattrCreate,
    Read { data: Vec<u8> },
    Write { count: u32 },
    Clunk,
    Remove,
    Mkdir { qid: Qid },
    Link,
    Readdir { data: Vec<u8> },
    RenameAt,
    UnlinkAt,
    Symlink { qid: Qid },
    Readlink { target: String },
    Fsync,
    Flush,
}

pub fn encode_request(tag: u16, request: &NinePRequest) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let typ = match request {
        NinePRequest::Version { msize, version } => {
            put_u32(&mut body, *msize);
            put_string(&mut body, version)?;
            msg::TVERSION
        }
        NinePRequest::Attach {
            fid,
            afid,
            uname,
            aname,
            n_uname,
        } => {
            put_u32(&mut body, *fid);
            put_u32(&mut body, *afid);
            put_string(&mut body, uname)?;
            put_string(&mut body, aname)?;
            put_u32(&mut body, *n_uname);
            msg::TATTACH
        }
        NinePRequest::Walk { fid, newfid, names } => {
            put_u32(&mut body, *fid);
            put_u32(&mut body, *newfid);
            put_u16(
                &mut body,
                names.len().try_into().map_err(|_| ErrorKind::Invalid)?,
            );
            for name in names {
                put_string(&mut body, name)?;
            }
            msg::TWALK
        }
        NinePRequest::Lopen { fid, flags } => {
            put_u32(&mut body, *fid);
            put_u32(&mut body, *flags);
            msg::TLOPEN
        }
        NinePRequest::Lcreate {
            fid,
            name,
            flags,
            mode,
            gid,
        } => {
            put_u32(&mut body, *fid);
            put_string(&mut body, name)?;
            put_u32(&mut body, *flags);
            put_u32(&mut body, *mode);
            put_u32(&mut body, *gid);
            msg::TLCREATE
        }
        NinePRequest::GetAttr { fid, request_mask } => {
            put_u32(&mut body, *fid);
            put_u64(&mut body, *request_mask);
            msg::TGETATTR
        }
        NinePRequest::SetAttr { fid, attr } => {
            put_u32(&mut body, *fid);
            encode_setattr(&mut body, attr);
            msg::TSETATTR
        }
        NinePRequest::XattrWalk { fid, newfid, name } => {
            put_u32(&mut body, *fid);
            put_u32(&mut body, *newfid);
            put_string(&mut body, name)?;
            msg::TXATTRWALK
        }
        NinePRequest::XattrCreate {
            fid,
            name,
            size,
            flags,
        } => {
            put_u32(&mut body, *fid);
            put_string(&mut body, name)?;
            put_u64(&mut body, *size);
            put_u32(&mut body, *flags);
            msg::TXATTRCREATE
        }
        NinePRequest::Read { fid, offset, count } => {
            put_u32(&mut body, *fid);
            put_u64(&mut body, *offset);
            put_u32(&mut body, *count);
            msg::TREAD
        }
        NinePRequest::Write { fid, offset, data } => {
            put_u32(&mut body, *fid);
            put_u64(&mut body, *offset);
            put_counted_data(&mut body, data)?;
            msg::TWRITE
        }
        NinePRequest::Clunk { fid } => {
            put_u32(&mut body, *fid);
            msg::TCLUNK
        }
        NinePRequest::Remove { fid } => {
            put_u32(&mut body, *fid);
            msg::TREMOVE
        }
        NinePRequest::Mkdir {
            fid,
            name,
            mode,
            gid,
        } => {
            put_u32(&mut body, *fid);
            put_string(&mut body, name)?;
            put_u32(&mut body, *mode);
            put_u32(&mut body, *gid);
            msg::TMKDIR
        }
        NinePRequest::Link {
            fid,
            newdirfid,
            name,
        } => {
            put_u32(&mut body, *fid);
            put_u32(&mut body, *newdirfid);
            put_string(&mut body, name)?;
            msg::TLINK
        }
        NinePRequest::Readdir { fid, offset, count } => {
            put_u32(&mut body, *fid);
            put_u64(&mut body, *offset);
            put_u32(&mut body, *count);
            msg::TREADDIR
        }
        NinePRequest::RenameAt {
            olddirfid,
            oldname,
            newdirfid,
            newname,
        } => {
            put_u32(&mut body, *olddirfid);
            put_string(&mut body, oldname)?;
            put_u32(&mut body, *newdirfid);
            put_string(&mut body, newname)?;
            msg::TRENAMEAT
        }
        NinePRequest::UnlinkAt {
            dirfid,
            name,
            flags,
        } => {
            put_u32(&mut body, *dirfid);
            put_string(&mut body, name)?;
            put_u32(&mut body, *flags);
            msg::TUNLINKAT
        }
        NinePRequest::Symlink {
            fid,
            name,
            target,
            gid,
        } => {
            put_u32(&mut body, *fid);
            put_string(&mut body, name)?;
            put_string(&mut body, target)?;
            put_u32(&mut body, *gid);
            msg::TSYMLINK
        }
        NinePRequest::Readlink { fid } => {
            put_u32(&mut body, *fid);
            msg::TREADLINK
        }
        NinePRequest::Fsync { fid } => {
            put_u32(&mut body, *fid);
            msg::TFSYNC
        }
        NinePRequest::Flush { oldtag } => {
            put_u16(&mut body, *oldtag);
            msg::TFLUSH
        }
    };
    encode_frame(typ, tag, &body)
}

pub fn decode_request(frame: &[u8]) -> Result<(u16, NinePRequest)> {
    let (typ, tag, mut d) = decode_frame(frame)?;
    let request = match typ {
        msg::TVERSION => NinePRequest::Version {
            msize: d.u32()?,
            version: d.string()?,
        },
        msg::TATTACH => NinePRequest::Attach {
            fid: d.u32()?,
            afid: d.u32()?,
            uname: d.string()?,
            aname: d.string()?,
            n_uname: d.u32()?,
        },
        msg::TWALK => {
            let fid = d.u32()?;
            let newfid = d.u32()?;
            let n = d.u16()? as usize;
            let mut names = Vec::with_capacity(n);
            for _ in 0..n {
                names.push(d.string()?);
            }
            NinePRequest::Walk { fid, newfid, names }
        }
        msg::TLOPEN => NinePRequest::Lopen {
            fid: d.u32()?,
            flags: d.u32()?,
        },
        msg::TLCREATE => NinePRequest::Lcreate {
            fid: d.u32()?,
            name: d.string()?,
            flags: d.u32()?,
            mode: d.u32()?,
            gid: d.u32()?,
        },
        msg::TGETATTR => NinePRequest::GetAttr {
            fid: d.u32()?,
            request_mask: d.u64()?,
        },
        msg::TSETATTR => NinePRequest::SetAttr {
            fid: d.u32()?,
            attr: d.setattr()?,
        },
        msg::TXATTRWALK => NinePRequest::XattrWalk {
            fid: d.u32()?,
            newfid: d.u32()?,
            name: d.string()?,
        },
        msg::TXATTRCREATE => NinePRequest::XattrCreate {
            fid: d.u32()?,
            name: d.string()?,
            size: d.u64()?,
            flags: d.u32()?,
        },
        msg::TREAD => NinePRequest::Read {
            fid: d.u32()?,
            offset: d.u64()?,
            count: d.u32()?,
        },
        msg::TWRITE => NinePRequest::Write {
            fid: d.u32()?,
            offset: d.u64()?,
            data: d.counted_data()?,
        },
        msg::TCLUNK => NinePRequest::Clunk { fid: d.u32()? },
        msg::TREMOVE => NinePRequest::Remove { fid: d.u32()? },
        msg::TMKDIR => NinePRequest::Mkdir {
            fid: d.u32()?,
            name: d.string()?,
            mode: d.u32()?,
            gid: d.u32()?,
        },
        msg::TLINK => NinePRequest::Link {
            fid: d.u32()?,
            newdirfid: d.u32()?,
            name: d.string()?,
        },
        msg::TREADDIR => NinePRequest::Readdir {
            fid: d.u32()?,
            offset: d.u64()?,
            count: d.u32()?,
        },
        msg::TRENAMEAT => NinePRequest::RenameAt {
            olddirfid: d.u32()?,
            oldname: d.string()?,
            newdirfid: d.u32()?,
            newname: d.string()?,
        },
        msg::TUNLINKAT => NinePRequest::UnlinkAt {
            dirfid: d.u32()?,
            name: d.string()?,
            flags: d.u32()?,
        },
        msg::TSYMLINK => NinePRequest::Symlink {
            fid: d.u32()?,
            name: d.string()?,
            target: d.string()?,
            gid: d.u32()?,
        },
        msg::TREADLINK => NinePRequest::Readlink { fid: d.u32()? },
        msg::TFSYNC => NinePRequest::Fsync { fid: d.u32()? },
        msg::TFLUSH => NinePRequest::Flush { oldtag: d.u16()? },
        _ => return Err(Error::Message(format!("unsupported 9P request type {typ}"))),
    };
    d.finish()?;
    Ok((tag, request))
}

pub fn encode_response(tag: u16, response: &NinePResponse) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let typ = match response {
        NinePResponse::Lerror { ecode } => {
            put_u32(&mut body, *ecode);
            msg::RLERROR
        }
        NinePResponse::Version { msize, version } => {
            put_u32(&mut body, *msize);
            put_string(&mut body, version)?;
            msg::RVERSION
        }
        NinePResponse::Attach { qid } => {
            encode_qid(&mut body, qid);
            msg::RATTACH
        }
        NinePResponse::Walk { qids } => {
            put_u16(
                &mut body,
                qids.len().try_into().map_err(|_| ErrorKind::Invalid)?,
            );
            for qid in qids {
                encode_qid(&mut body, qid);
            }
            msg::RWALK
        }
        NinePResponse::Lopen { qid, iounit } => {
            encode_qid(&mut body, qid);
            put_u32(&mut body, *iounit);
            msg::RLOPEN
        }
        NinePResponse::Lcreate { qid, iounit } => {
            encode_qid(&mut body, qid);
            put_u32(&mut body, *iounit);
            msg::RLCREATE
        }
        NinePResponse::GetAttr { attr } => {
            encode_attr(&mut body, attr);
            msg::RGETATTR
        }
        NinePResponse::SetAttr => msg::RSETATTR,
        NinePResponse::XattrWalk { size } => {
            put_u64(&mut body, *size);
            msg::RXATTRWALK
        }
        NinePResponse::XattrCreate => msg::RXATTRCREATE,
        NinePResponse::Read { data } => {
            put_counted_data(&mut body, data)?;
            msg::RREAD
        }
        NinePResponse::Write { count } => {
            put_u32(&mut body, *count);
            msg::RWRITE
        }
        NinePResponse::Clunk => msg::RCLUNK,
        NinePResponse::Remove => msg::RREMOVE,
        NinePResponse::Mkdir { qid } => {
            encode_qid(&mut body, qid);
            msg::RMKDIR
        }
        NinePResponse::Link => msg::RLINK,
        NinePResponse::Readdir { data } => {
            put_counted_data(&mut body, data)?;
            msg::RREADDIR
        }
        NinePResponse::RenameAt => msg::RRENAMEAT,
        NinePResponse::UnlinkAt => msg::RUNLINKAT,
        NinePResponse::Symlink { qid } => {
            encode_qid(&mut body, qid);
            msg::RSYMLINK
        }
        NinePResponse::Readlink { target } => {
            put_string(&mut body, target)?;
            msg::RREADLINK
        }
        NinePResponse::Fsync => msg::RFSYNC,
        NinePResponse::Flush => msg::RFLUSH,
    };
    encode_frame(typ, tag, &body)
}

pub fn decode_response(frame: &[u8]) -> Result<(u16, NinePResponse)> {
    let (typ, tag, mut d) = decode_frame(frame)?;
    let response = match typ {
        msg::RLERROR => NinePResponse::Lerror { ecode: d.u32()? },
        msg::RVERSION => NinePResponse::Version {
            msize: d.u32()?,
            version: d.string()?,
        },
        msg::RATTACH => NinePResponse::Attach { qid: d.qid()? },
        msg::RWALK => {
            let n = d.u16()? as usize;
            let mut qids = Vec::with_capacity(n);
            for _ in 0..n {
                qids.push(d.qid()?);
            }
            NinePResponse::Walk { qids }
        }
        msg::RLOPEN => NinePResponse::Lopen {
            qid: d.qid()?,
            iounit: d.u32()?,
        },
        msg::RLCREATE => NinePResponse::Lcreate {
            qid: d.qid()?,
            iounit: d.u32()?,
        },
        msg::RGETATTR => NinePResponse::GetAttr { attr: d.attr()? },
        msg::RSETATTR => NinePResponse::SetAttr,
        msg::RXATTRWALK => NinePResponse::XattrWalk { size: d.u64()? },
        msg::RXATTRCREATE => NinePResponse::XattrCreate,
        msg::RREAD => NinePResponse::Read {
            data: d.counted_data()?,
        },
        msg::RWRITE => NinePResponse::Write { count: d.u32()? },
        msg::RCLUNK => NinePResponse::Clunk,
        msg::RREMOVE => NinePResponse::Remove,
        msg::RMKDIR => NinePResponse::Mkdir { qid: d.qid()? },
        msg::RLINK => NinePResponse::Link,
        msg::RREADDIR => NinePResponse::Readdir {
            data: d.counted_data()?,
        },
        msg::RRENAMEAT => NinePResponse::RenameAt,
        msg::RUNLINKAT => NinePResponse::UnlinkAt,
        msg::RSYMLINK => NinePResponse::Symlink { qid: d.qid()? },
        msg::RREADLINK => NinePResponse::Readlink {
            target: d.string()?,
        },
        msg::RFSYNC => NinePResponse::Fsync,
        msg::RFLUSH => NinePResponse::Flush,
        _ => {
            return Err(Error::Message(format!(
                "unsupported 9P response type {typ}"
            )))
        }
    };
    d.finish()?;
    Ok((tag, response))
}

pub fn encode_dirents(dirents: &[NinePDirent]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for dirent in dirents {
        encode_qid(&mut out, &dirent.qid);
        put_u64(&mut out, dirent.offset);
        out.push(dirent.typ);
        put_string(&mut out, &dirent.name)?;
    }
    Ok(out)
}

pub fn decode_dirents(data: &[u8]) -> Result<Vec<NinePDirent>> {
    let mut d = Decoder::new(data);
    let mut out = Vec::new();
    while !d.is_empty() {
        out.push(NinePDirent {
            qid: d.qid()?,
            offset: d.u64()?,
            typ: d.u8()?,
            name: d.string()?,
        });
    }
    Ok(out)
}

fn encode_frame(typ: u8, tag: u16, body: &[u8]) -> Result<Vec<u8>> {
    let size = 7usize
        .checked_add(body.len())
        .and_then(|size| u32::try_from(size).ok())
        .ok_or(ErrorKind::Invalid)?;
    let mut out = Vec::with_capacity(size as usize);
    put_u32(&mut out, size);
    out.push(typ);
    put_u16(&mut out, tag);
    out.extend_from_slice(body);
    Ok(out)
}

fn decode_frame(frame: &[u8]) -> Result<(u8, u16, Decoder<'_>)> {
    if frame.len() < 7 {
        return Err(ErrorKind::Invalid.into());
    }
    let size = u32::from_le_bytes(frame[0..4].try_into().unwrap()) as usize;
    if size != frame.len() || size < 7 {
        return Err(ErrorKind::Invalid.into());
    }
    let typ = frame[4];
    let tag = u16::from_le_bytes(frame[5..7].try_into().unwrap());
    Ok((typ, tag, Decoder::new(&frame[7..])))
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_string(out: &mut Vec<u8>, value: &str) -> Result<()> {
    let len: u16 = value.len().try_into().map_err(|_| ErrorKind::Invalid)?;
    put_u16(out, len);
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn put_counted_data(out: &mut Vec<u8>, data: &[u8]) -> Result<()> {
    let len: u32 = data.len().try_into().map_err(|_| ErrorKind::Invalid)?;
    put_u32(out, len);
    out.extend_from_slice(data);
    Ok(())
}

fn encode_qid(out: &mut Vec<u8>, qid: &Qid) {
    out.push(qid.typ);
    put_u32(out, qid.version);
    put_u64(out, qid.path);
}

fn encode_attr(out: &mut Vec<u8>, attr: &NinePAttr) {
    put_u64(out, attr.valid);
    encode_qid(out, &attr.qid);
    put_u32(out, attr.mode);
    put_u32(out, attr.uid);
    put_u32(out, attr.gid);
    put_u64(out, attr.nlink);
    put_u64(out, attr.rdev);
    put_u64(out, attr.size);
    put_u64(out, attr.blksize);
    put_u64(out, attr.blocks);
    put_u64(out, attr.atime_seconds);
    put_u64(out, attr.atime_nanoseconds);
    put_u64(out, attr.mtime_seconds);
    put_u64(out, attr.mtime_nanoseconds);
    put_u64(out, attr.ctime_seconds);
    put_u64(out, attr.ctime_nanoseconds);
    put_u64(out, attr.btime_seconds);
    put_u64(out, attr.btime_nanoseconds);
    put_u64(out, attr.gen);
    put_u64(out, attr.data_version);
}

fn encode_setattr(out: &mut Vec<u8>, attr: &SetAttr) {
    put_u32(out, attr.valid);
    put_u32(out, attr.mode);
    put_u32(out, attr.uid);
    put_u32(out, attr.gid);
    put_u64(out, attr.size);
    put_u64(out, attr.atime_seconds);
    put_u64(out, attr.atime_nanoseconds);
    put_u64(out, attr.mtime_seconds);
    put_u64(out, attr.mtime_nanoseconds);
}

struct Decoder<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn is_empty(&self) -> bool {
        self.offset == self.data.len()
    }

    fn finish(&self) -> Result<()> {
        if self.is_empty() {
            Ok(())
        } else {
            Err(ErrorKind::Invalid.into())
        }
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.offset.checked_add(len).ok_or(ErrorKind::Invalid)?;
        let bytes = self
            .data
            .get(self.offset..end)
            .ok_or(ErrorKind::UnexpectedEof)?;
        self.offset = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(
            self.bytes(2)?.try_into().map_err(|_| ErrorKind::Invalid)?,
        ))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.bytes(4)?.try_into().map_err(|_| ErrorKind::Invalid)?,
        ))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.bytes(8)?.try_into().map_err(|_| ErrorKind::Invalid)?,
        ))
    }

    fn string(&mut self) -> Result<String> {
        let len = self.u16()? as usize;
        let bytes = self.bytes(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ErrorKind::Invalid.into())
    }

    fn counted_data(&mut self) -> Result<Vec<u8>> {
        let len = self.u32()? as usize;
        Ok(self.bytes(len)?.to_vec())
    }

    fn qid(&mut self) -> Result<Qid> {
        Ok(Qid {
            typ: self.u8()?,
            version: self.u32()?,
            path: self.u64()?,
        })
    }

    fn attr(&mut self) -> Result<NinePAttr> {
        Ok(NinePAttr {
            valid: self.u64()?,
            qid: self.qid()?,
            mode: self.u32()?,
            uid: self.u32()?,
            gid: self.u32()?,
            nlink: self.u64()?,
            rdev: self.u64()?,
            size: self.u64()?,
            blksize: self.u64()?,
            blocks: self.u64()?,
            atime_seconds: self.u64()?,
            atime_nanoseconds: self.u64()?,
            mtime_seconds: self.u64()?,
            mtime_nanoseconds: self.u64()?,
            ctime_seconds: self.u64()?,
            ctime_nanoseconds: self.u64()?,
            btime_seconds: self.u64()?,
            btime_nanoseconds: self.u64()?,
            gen: self.u64()?,
            data_version: self.u64()?,
        })
    }

    fn setattr(&mut self) -> Result<SetAttr> {
        Ok(SetAttr {
            valid: self.u32()?,
            mode: self.u32()?,
            uid: self.u32()?,
            gid: self.u32()?,
            size: self.u64()?,
            atime_seconds: self.u64()?,
            atime_nanoseconds: self.u64()?,
            mtime_seconds: self.u64()?,
            mtime_nanoseconds: self.u64()?,
        })
    }
}

struct FidState {
    path: String,
    handle: FidHandle,
    flags: u32,
}

impl FidState {
    fn unopened(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            handle: FidHandle::None,
            flags: 0,
        }
    }
}

enum FidHandle {
    None,
    File(BoxFile),
    XattrRead { data: Vec<u8> },
    XattrWrite(XattrWriteState),
}

struct XattrWriteState {
    name: String,
    flags: u32,
    expected_size: u64,
    data: Vec<u8>,
    written: Vec<Range<u64>>,
    committed: bool,
}

impl XattrWriteState {
    fn new(name: String, flags: u32, expected_size: u64) -> Result<Self> {
        Ok(Self {
            name,
            flags,
            expected_size,
            data: vec![0; expected_size.try_into().map_err(|_| ErrorKind::Invalid)?],
            written: Vec::new(),
            committed: false,
        })
    }

    fn write_at(&mut self, offset: u64, chunk: &[u8]) -> Result<usize> {
        let end = offset
            .checked_add(chunk.len().try_into().map_err(|_| ErrorKind::Invalid)?)
            .ok_or(ErrorKind::Invalid)?;
        if end > self.expected_size {
            return Err(ErrorKind::Invalid.into());
        }
        let start: usize = offset.try_into().map_err(|_| ErrorKind::Invalid)?;
        let end: usize = end.try_into().map_err(|_| ErrorKind::Invalid)?;
        self.data[start..end].copy_from_slice(chunk);
        self.record_write(offset..end as u64);
        Ok(chunk.len())
    }

    fn is_complete(&self) -> bool {
        self.expected_size == 0
            || matches!(
                self.written.as_slice(),
                [range] if range.start == 0 && range.end == self.expected_size
            )
    }

    fn record_write(&mut self, range: Range<u64>) {
        if range.is_empty() {
            return;
        }

        let mut start = range.start;
        let mut end = range.end;
        let mut merged = Vec::with_capacity(self.written.len() + 1);
        let mut inserted = false;
        for existing in self.written.drain(..) {
            if existing.end < start {
                merged.push(existing);
            } else if end < existing.start {
                if !inserted {
                    merged.push(start..end);
                    inserted = true;
                }
                merged.push(existing);
            } else {
                start = start.min(existing.start);
                end = end.max(existing.end);
            }
        }
        if !inserted {
            merged.push(start..end);
        }
        self.written = merged;
    }
}

struct ServerState {
    msize: u32,
    fids: HashMap<u32, FidState>,
}

pub struct NinePServer {
    fsys: FsRef,
    state: Mutex<ServerState>,
}

impl NinePServer {
    pub fn new(fsys: FsRef) -> Self {
        Self {
            fsys,
            state: Mutex::new(ServerState {
                msize: DEFAULT_MSIZE,
                fids: HashMap::new(),
            }),
        }
    }

    pub fn handle_frame(&self, frame: &[u8]) -> Result<Vec<u8>> {
        let (tag, request) = decode_request(frame)?;
        let response = self
            .handle(request)
            .unwrap_or_else(|err| NinePResponse::Lerror {
                ecode: errno_for_error(&err),
            });
        encode_response(tag, &response)
    }

    fn handle(&self, request: NinePRequest) -> Result<NinePResponse> {
        match request {
            NinePRequest::Version { msize, version } => {
                let negotiated = msize.min(DEFAULT_MSIZE);
                self.state.lock().unwrap().msize = negotiated;
                Ok(NinePResponse::Version {
                    msize: negotiated,
                    version: if version.starts_with("9P2000") {
                        VERSION.to_string()
                    } else {
                        "unknown".to_string()
                    },
                })
            }
            NinePRequest::Attach { fid, .. } => {
                let qid = qid_for(self.fsys.as_ref(), ".")?;
                self.state
                    .lock()
                    .unwrap()
                    .fids
                    .insert(fid, FidState::unopened("."));
                Ok(NinePResponse::Attach { qid })
            }
            NinePRequest::Walk { fid, newfid, names } => self.walk(fid, newfid, names),
            NinePRequest::Lopen { fid, flags } => self.lopen(fid, flags),
            NinePRequest::Lcreate {
                fid,
                name,
                flags,
                mode,
                ..
            } => self.lcreate(fid, &name, flags, mode),
            NinePRequest::GetAttr { fid, request_mask } => self.getattr(fid, request_mask),
            NinePRequest::SetAttr { fid, attr } => self.setattr(fid, attr),
            NinePRequest::XattrWalk { fid, newfid, name } => self.xattrwalk(fid, newfid, &name),
            NinePRequest::XattrCreate {
                fid,
                name,
                size,
                flags,
            } => self.xattrcreate(fid, &name, size, flags),
            NinePRequest::Read { fid, offset, count } => self.read(fid, offset, count),
            NinePRequest::Write { fid, offset, data } => self.write(fid, offset, &data),
            NinePRequest::Clunk { fid } => self.clunk(fid).map(|_| NinePResponse::Clunk),
            NinePRequest::Remove { fid } => self.remove(fid),
            NinePRequest::Mkdir {
                fid, name, mode, ..
            } => self.mkdir(fid, &name, mode),
            NinePRequest::Link {
                fid,
                newdirfid,
                name,
            } => self.link(fid, newdirfid, &name),
            NinePRequest::Readdir { fid, offset, count } => self.readdir(fid, offset, count),
            NinePRequest::RenameAt {
                olddirfid,
                oldname,
                newdirfid,
                newname,
            } => self.rename_at(olddirfid, &oldname, newdirfid, &newname),
            NinePRequest::UnlinkAt {
                dirfid,
                name,
                flags,
            } => self.unlink_at(dirfid, &name, flags),
            NinePRequest::Symlink {
                fid, name, target, ..
            } => self.symlink(fid, &name, &target),
            NinePRequest::Readlink { fid } => self.readlink(fid),
            NinePRequest::Fsync { fid } => self.fsync(fid),
            NinePRequest::Flush { oldtag } => Ok(self.flush(oldtag)),
        }
    }

    fn walk(&self, fid: u32, newfid: u32, names: Vec<String>) -> Result<NinePResponse> {
        let base = {
            let state = self.state.lock().unwrap();
            if newfid != fid && state.fids.contains_key(&newfid) {
                return Err(ErrorKind::Invalid.into());
            }
            state.fids.get(&fid).ok_or(ErrorKind::Invalid)?.path.clone()
        };
        let mut path = base;
        let mut qids = Vec::with_capacity(names.len());
        for name in names {
            if name.contains('/') || name.is_empty() {
                return Err(ErrorKind::Invalid.into());
            }
            let next_path = walk_join(&path, &name);
            match qid_for(self.fsys.as_ref(), &next_path) {
                Ok(qid) => {
                    path = next_path;
                    qids.push(qid);
                }
                Err(err) if qids.is_empty() => return Err(err),
                Err(_) => break,
            }
        }
        self.state
            .lock()
            .unwrap()
            .fids
            .insert(newfid, FidState::unopened(path));
        Ok(NinePResponse::Walk { qids })
    }

    fn lopen(&self, fid: u32, flags: u32) -> Result<NinePResponse> {
        let path = self.fid_path(fid)?;
        let file =
            self.fsys
                .open_file(&path, open_flags_from_9p(flags), FileMode::from_perm(0o666))?;
        let qid = qid_for(self.fsys.as_ref(), &path)?;
        let mut state = self.state.lock().unwrap();
        let fid_state = state.fids.get_mut(&fid).ok_or(ErrorKind::Invalid)?;
        fid_state.handle = FidHandle::File(file);
        fid_state.flags = flags;
        Ok(NinePResponse::Lopen { qid, iounit: 0 })
    }

    fn lcreate(&self, fid: u32, name: &str, flags: u32, mode: u32) -> Result<NinePResponse> {
        let parent = self.fid_path(fid)?;
        let path = checked_join(&parent, name)?;
        let perm = FileMode::from_perm(mode & 0o777);
        let file =
            self.fsys
                .open_file(&path, open_flags_from_9p(flags) | OpenFlags::CREATE, perm)?;
        let qid = qid_for(self.fsys.as_ref(), &path)?;
        let mut state = self.state.lock().unwrap();
        let fid_state = state.fids.get_mut(&fid).ok_or(ErrorKind::Invalid)?;
        fid_state.path = path;
        fid_state.handle = FidHandle::File(file);
        fid_state.flags = flags;
        Ok(NinePResponse::Lcreate { qid, iounit: 0 })
    }

    fn getattr(&self, fid: u32, request_mask: u64) -> Result<NinePResponse> {
        let (path, meta) = {
            let mut state = self.state.lock().unwrap();
            let fid_state = state.fids.get_mut(&fid).ok_or(ErrorKind::Invalid)?;
            let meta = match &mut fid_state.handle {
                FidHandle::File(file) => file.stat()?,
                FidHandle::None => self
                    .fsys
                    .lstat(&FsContext::new().no_follow(), &fid_state.path)?,
                FidHandle::XattrRead { data } => Metadata::file("xattr", 0o444, data.len() as u64),
                FidHandle::XattrWrite(write) => Metadata::file("xattr", 0o200, write.expected_size),
            };
            (fid_state.path.clone(), meta)
        };
        Ok(NinePResponse::GetAttr {
            attr: attr_for(self.fsys.as_ref(), &path, &meta, request_mask),
        })
    }

    fn setattr(&self, fid: u32, attr: SetAttr) -> Result<NinePResponse> {
        let path = self.fid_path(fid)?;
        if attr.valid & SETATTR_SIZE != 0 {
            self.fsys.truncate(&path, attr.size)?;
        }
        if attr.valid & SETATTR_MODE != 0 {
            self.fsys
                .chmod(&path, FileMode::from_perm(attr.mode & 0o777))?;
        }
        if attr.valid & (SETATTR_UID | SETATTR_GID) != 0 {
            let meta = self.fsys.lstat(&FsContext::new().no_follow(), &path)?;
            let uid = if attr.valid & SETATTR_UID != 0 {
                attr.uid
            } else {
                meta.uid
            };
            let gid = if attr.valid & SETATTR_GID != 0 {
                attr.gid
            } else {
                meta.gid
            };
            self.fsys.chown(&path, uid, gid)?;
        }
        if attr.valid & SETATTR_MTIME != 0 {
            let mtime = if attr.valid & SETATTR_MTIME_SET != 0 {
                SystemTime::UNIX_EPOCH
                    + Duration::new(attr.mtime_seconds, attr.mtime_nanoseconds as u32)
            } else {
                current_time()
            };
            self.fsys.chtimes(&path, mtime)?;
        }
        Ok(NinePResponse::SetAttr)
    }

    fn xattrwalk(&self, fid: u32, newfid: u32, name: &str) -> Result<NinePResponse> {
        let path = {
            let state = self.state.lock().unwrap();
            if newfid != fid && state.fids.contains_key(&newfid) {
                return Err(ErrorKind::Invalid.into());
            }
            state.fids.get(&fid).ok_or(ErrorKind::Invalid)?.path.clone()
        };
        let data = if name.is_empty() {
            encode_xattr_names(&self.fsys.list_xattrs(&path)?)
        } else {
            self.fsys.get_xattr(&path, name)?
        };
        self.state.lock().unwrap().fids.insert(
            newfid,
            FidState {
                path,
                handle: FidHandle::XattrRead { data: data.clone() },
                flags: OpenFlags::RDONLY.bits(),
            },
        );
        Ok(NinePResponse::XattrWalk {
            size: data.len().try_into().map_err(|_| ErrorKind::Invalid)?,
        })
    }

    fn xattrcreate(&self, fid: u32, name: &str, size: u64, flags: u32) -> Result<NinePResponse> {
        if name.is_empty() {
            return Err(ErrorKind::Invalid.into());
        }
        let mut state = self.state.lock().unwrap();
        let fid_state = state.fids.get_mut(&fid).ok_or(ErrorKind::Invalid)?;
        fid_state.handle =
            FidHandle::XattrWrite(XattrWriteState::new(name.to_string(), flags, size)?);
        fid_state.flags = OpenFlags::WRONLY.bits();
        Ok(NinePResponse::XattrCreate)
    }

    fn read(&self, fid: u32, offset: u64, count: u32) -> Result<NinePResponse> {
        let mut state = self.state.lock().unwrap();
        let fid_state = state.fids.get_mut(&fid).ok_or(ErrorKind::Invalid)?;
        let data = match &mut fid_state.handle {
            FidHandle::File(file) => {
                let mut data = vec![0; count as usize];
                let n = file.read_at(&mut data, offset)?;
                data.truncate(n);
                data
            }
            FidHandle::XattrRead { data } => {
                let start: usize = offset.try_into().map_err(|_| ErrorKind::Invalid)?;
                if start >= data.len() {
                    Vec::new()
                } else {
                    data[start..start.saturating_add(count as usize).min(data.len())].to_vec()
                }
            }
            FidHandle::XattrWrite(_) => return Err(ErrorKind::PermissionDenied.into()),
            FidHandle::None => return Err(ErrorKind::Invalid.into()),
        };
        Ok(NinePResponse::Read { data })
    }

    fn write(&self, fid: u32, offset: u64, data: &[u8]) -> Result<NinePResponse> {
        let mut state = self.state.lock().unwrap();
        let fid_state = state.fids.get_mut(&fid).ok_or(ErrorKind::Invalid)?;
        let count = match &mut fid_state.handle {
            FidHandle::File(file) => file.write_at(data, offset)?,
            FidHandle::XattrWrite(write) => write.write_at(offset, data)?,
            FidHandle::XattrRead { .. } => return Err(ErrorKind::PermissionDenied.into()),
            FidHandle::None => return Err(ErrorKind::Invalid.into()),
        };
        Ok(NinePResponse::Write {
            count: count.try_into().map_err(|_| ErrorKind::Invalid)?,
        })
    }

    fn clunk(&self, fid: u32) -> Result<()> {
        let state = {
            let mut state = self.state.lock().unwrap();
            state.fids.remove(&fid)
        };
        let mut fid_state = state.ok_or(ErrorKind::Invalid)?;
        self.commit_xattr_if_ready(&mut fid_state)?;
        if let FidHandle::File(mut file) = std::mem::replace(&mut fid_state.handle, FidHandle::None)
        {
            file.close()?;
        }
        Ok(())
    }

    fn remove(&self, fid: u32) -> Result<NinePResponse> {
        let state = {
            let mut state = self.state.lock().unwrap();
            state.fids.remove(&fid)
        };
        let mut fid_state = state.ok_or(ErrorKind::Invalid)?;
        if fid_state.path == "." {
            return Err(ErrorKind::Invalid.into());
        }
        if let FidHandle::File(mut file) = std::mem::replace(&mut fid_state.handle, FidHandle::None)
        {
            let _ = file.close();
        }
        self.fsys.remove(&fid_state.path)?;
        Ok(NinePResponse::Remove)
    }

    fn mkdir(&self, fid: u32, name: &str, mode: u32) -> Result<NinePResponse> {
        let parent = self.fid_path(fid)?;
        let path = checked_join(&parent, name)?;
        self.fsys
            .mkdir(&path, FileMode::DIR | FileMode::from_perm(mode & 0o777))?;
        Ok(NinePResponse::Mkdir {
            qid: qid_for(self.fsys.as_ref(), &path)?,
        })
    }

    fn link(&self, fid: u32, newdirfid: u32, name: &str) -> Result<NinePResponse> {
        let old = self.fid_path(fid)?;
        let new = checked_join(&self.fid_path(newdirfid)?, name)?;
        self.fsys.link(&old, &new)?;
        Ok(NinePResponse::Link)
    }

    fn readdir(&self, fid: u32, offset: u64, count: u32) -> Result<NinePResponse> {
        let path = self.fid_path(fid)?;
        let entries = self.fsys.read_dir(&FsContext::new(), &path)?;
        let mut data = Vec::new();
        for (index, entry) in entries.into_iter().enumerate() {
            let cursor = index as u64 + 1;
            if cursor <= offset {
                continue;
            }
            let child = join_path(&path, &entry.name);
            let encoded = encode_dirents(&[NinePDirent {
                qid: qid_for(self.fsys.as_ref(), &child)?,
                offset: cursor,
                typ: dirent_type(entry.metadata.mode),
                name: entry.name,
            }])?;
            if data.len().saturating_add(encoded.len()) > count as usize {
                break;
            }
            data.extend(encoded);
        }
        Ok(NinePResponse::Readdir { data })
    }

    fn rename_at(
        &self,
        olddirfid: u32,
        oldname: &str,
        newdirfid: u32,
        newname: &str,
    ) -> Result<NinePResponse> {
        let old = checked_join(&self.fid_path(olddirfid)?, oldname)?;
        let new = checked_join(&self.fid_path(newdirfid)?, newname)?;
        self.fsys.rename(&old, &new)?;
        Ok(NinePResponse::RenameAt)
    }

    fn unlink_at(&self, dirfid: u32, name: &str, flags: u32) -> Result<NinePResponse> {
        let path = checked_join(&self.fid_path(dirfid)?, name)?;
        let meta = self.fsys.lstat(&FsContext::new().no_follow(), &path)?;
        if meta.is_dir() && flags & AT_REMOVEDIR == 0 {
            return Err(ErrorKind::IsDir.into());
        }
        if !meta.is_dir() && flags & AT_REMOVEDIR != 0 {
            return Err(ErrorKind::NotDir.into());
        }
        if meta.is_dir() && !self.fsys.read_dir(&FsContext::new(), &path)?.is_empty() {
            return Err(ErrorKind::NotEmpty.into());
        }
        self.fsys.remove(&path)?;
        Ok(NinePResponse::UnlinkAt)
    }

    fn symlink(&self, fid: u32, name: &str, target: &str) -> Result<NinePResponse> {
        let path = checked_join(&self.fid_path(fid)?, name)?;
        self.fsys.symlink(target, &path)?;
        Ok(NinePResponse::Symlink {
            qid: qid_for(self.fsys.as_ref(), &path)?,
        })
    }

    fn readlink(&self, fid: u32) -> Result<NinePResponse> {
        let path = self.fid_path(fid)?;
        Ok(NinePResponse::Readlink {
            target: self.fsys.readlink(&path)?,
        })
    }

    fn fsync(&self, fid: u32) -> Result<NinePResponse> {
        let mut state = self.state.lock().unwrap();
        let fid_state = state.fids.get_mut(&fid).ok_or(ErrorKind::Invalid)?;
        let mut commit_xattr = false;
        match &mut fid_state.handle {
            FidHandle::File(file) => match file.sync() {
                Ok(()) | Err(Error::Kind(ErrorKind::NotSupported)) => {}
                Err(err) => return Err(err),
            },
            FidHandle::XattrWrite(_) => commit_xattr = true,
            FidHandle::XattrRead { .. } | FidHandle::None => {}
        }
        if commit_xattr {
            self.commit_xattr_if_ready(fid_state)?;
        }
        Ok(NinePResponse::Fsync)
    }

    fn commit_xattr_if_ready(&self, fid_state: &mut FidState) -> Result<()> {
        if let FidHandle::XattrWrite(write) = &mut fid_state.handle {
            if write.committed {
                return Ok(());
            }
            if !write.is_complete() {
                return Err(ErrorKind::Invalid.into());
            }
            let _ = write.flags;
            self.fsys
                .set_xattr(&fid_state.path, &write.name, &write.data)?;
            write.committed = true;
        }
        Ok(())
    }

    fn flush(&self, _oldtag: u16) -> NinePResponse {
        // Requests are handled synchronously, so flush is an acknowledgement-only no-op.
        NinePResponse::Flush
    }

    fn fid_path(&self, fid: u32) -> Result<String> {
        self.state
            .lock()
            .unwrap()
            .fids
            .get(&fid)
            .map(|state| state.path.clone())
            .ok_or(ErrorKind::Invalid.into())
    }
}

pub trait NinePTransport: Send + Sync {
    fn round_trip(&self, request: Vec<u8>) -> Result<Vec<u8>>;
}

pub fn read_frame(reader: &mut impl Read) -> Result<Option<Vec<u8>>> {
    let mut header = [0_u8; 4];
    match reader.read(&mut header[..1]) {
        Ok(0) => return Ok(None),
        Ok(1) => {}
        Ok(_) => unreachable!("single-byte read buffer cannot read more than one byte"),
        Err(err) => return Err(err.into()),
    }
    reader.read_exact(&mut header[1..])?;
    let size = u32::from_le_bytes(header) as usize;
    validate_frame_size(size)?;
    let mut frame = vec![0_u8; size];
    frame[..4].copy_from_slice(&header);
    reader.read_exact(&mut frame[4..])?;
    Ok(Some(frame))
}

pub fn write_frame(writer: &mut impl Write, frame: &[u8]) -> Result<()> {
    let size = encoded_frame_size(frame)?;
    validate_frame_size(size)?;
    if size != frame.len() {
        return Err(ErrorKind::Invalid.into());
    }
    writer.write_all(frame)?;
    writer.flush()?;
    Ok(())
}

pub fn serve_frame_stream(
    server: &NinePServer,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<usize> {
    let mut served = 0;
    while let Some(frame) = read_frame(reader)? {
        let response = server.handle_frame(&frame)?;
        write_frame(writer, &response)?;
        served += 1;
    }
    Ok(served)
}

#[derive(Clone)]
pub struct AsyncNinePServer {
    inner: Arc<AsyncNinePServerInner>,
}

struct AsyncNinePServerInner {
    server: Arc<NinePServer>,
    pending: Mutex<HashMap<u16, AsyncPending>>,
    responses: Mutex<VecDeque<Vec<u8>>>,
    responses_ready: Condvar,
}

#[derive(Clone)]
struct AsyncPending {
    cancelled: Arc<AtomicBool>,
}

impl AsyncNinePServer {
    pub fn new(fsys: FsRef) -> Self {
        Self::from_server(Arc::new(NinePServer::new(fsys)))
    }

    pub fn from_server(server: Arc<NinePServer>) -> Self {
        Self {
            inner: Arc::new(AsyncNinePServerInner {
                server,
                pending: Mutex::new(HashMap::new()),
                responses: Mutex::new(VecDeque::new()),
                responses_ready: Condvar::new(),
            }),
        }
    }

    pub fn handle_frame(&self, frame: &[u8]) -> Result<Option<Vec<u8>>> {
        let (tag, request) = decode_request(frame)?;
        if let NinePRequest::Flush { oldtag } = request {
            self.cancel(oldtag);
            return encode_response(tag, &NinePResponse::Flush).map(Some);
        }

        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut pending = self.inner.pending.lock().unwrap();
            if let Some(existing) = pending.remove(&tag) {
                existing.cancelled.store(true, Ordering::SeqCst);
                return encode_response(
                    tag,
                    &NinePResponse::Lerror {
                        ecode: errno_for_error(&ErrorKind::Invalid.into()),
                    },
                )
                .map(Some);
            }
            pending.insert(
                tag,
                AsyncPending {
                    cancelled: cancelled.clone(),
                },
            );
        }

        let frame = frame.to_vec();
        let inner = self.inner.clone();
        thread::spawn(move || {
            let response = inner.server.handle_frame(&frame).unwrap_or_else(|_| {
                encode_response(tag, &NinePResponse::Lerror { ecode: EIO })
                    .expect("9P error response encodes")
            });
            let should_send = {
                let mut pending = inner.pending.lock().unwrap();
                match pending.get(&tag) {
                    Some(state)
                        if Arc::ptr_eq(&state.cancelled, &cancelled)
                            && !state.cancelled.load(Ordering::SeqCst) =>
                    {
                        pending.remove(&tag);
                        true
                    }
                    _ => false,
                }
            };
            if should_send {
                let mut responses = inner.responses.lock().unwrap();
                responses.push_back(response);
                inner.responses_ready.notify_all();
            }
        });

        Ok(None)
    }

    pub fn cancel(&self, tag: u16) -> bool {
        let pending = self.inner.pending.lock().unwrap().remove(&tag);
        if let Some(pending) = pending {
            pending.cancelled.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub fn next_response(&self) -> Option<Vec<u8>> {
        self.inner.responses.lock().unwrap().pop_front()
    }

    pub fn recv_response_timeout(&self, timeout: Duration) -> Option<Vec<u8>> {
        let mut responses = self.inner.responses.lock().unwrap();
        if responses.is_empty() {
            let (next, _) = self
                .inner
                .responses_ready
                .wait_timeout(responses, timeout)
                .unwrap();
            responses = next;
        }
        responses.pop_front()
    }

    pub fn pending_tags(&self) -> Vec<u16> {
        let mut tags: Vec<_> = self.inner.pending.lock().unwrap().keys().copied().collect();
        tags.sort_unstable();
        tags
    }
}

fn encoded_frame_size(frame: &[u8]) -> Result<usize> {
    if frame.len() < 4 {
        return Err(ErrorKind::Invalid.into());
    }
    Ok(u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize)
}

fn validate_frame_size(size: usize) -> Result<()> {
    if !(7..=MAX_STREAM_FRAME_SIZE).contains(&size) {
        return Err(ErrorKind::Invalid.into());
    }
    Ok(())
}

pub struct LoopbackTransport {
    server: Arc<NinePServer>,
}

impl LoopbackTransport {
    pub fn new(server: Arc<NinePServer>) -> Self {
        Self { server }
    }

    pub fn with_filesystem(fsys: FsRef) -> Self {
        Self::new(Arc::new(NinePServer::new(fsys)))
    }
}

impl NinePTransport for LoopbackTransport {
    fn round_trip(&self, request: Vec<u8>) -> Result<Vec<u8>> {
        self.server.handle_frame(&request)
    }
}

pub struct StreamTransport<S> {
    stream: Mutex<S>,
}

impl<S> StreamTransport<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream: Mutex::new(stream),
        }
    }
}

impl<S> NinePTransport for StreamTransport<S>
where
    S: Read + Write + Send,
{
    fn round_trip(&self, request: Vec<u8>) -> Result<Vec<u8>> {
        let mut stream = self.stream.lock().unwrap();
        write_frame(&mut *stream, &request)?;
        read_frame(&mut *stream)?.ok_or_else(|| ErrorKind::UnexpectedEof.into())
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub type TcpStreamTransport = StreamTransport<std::net::TcpStream>;

#[derive(Clone)]
pub struct NinePClientFs {
    inner: Arc<NinePClientInner>,
}

struct NinePClientInner {
    transport: Arc<dyn NinePTransport>,
    msize: AtomicU32,
    next_tag: AtomicU16,
    next_fid: AtomicU32,
    root_fid: u32,
}

fn validated_client_path(op: &'static str, name: &str) -> Result<String> {
    if name.is_empty() || name.starts_with('/') || name.split('/').any(|part| part == "..") {
        return Err(Error::path(op, name, ErrorKind::Invalid));
    }
    let path = clean_path(name);
    if !valid_path(&path) {
        return Err(Error::path(op, path, ErrorKind::Invalid));
    }
    Ok(path)
}

impl NinePClientFs {
    pub fn connect(transport: Arc<dyn NinePTransport>) -> Result<Self> {
        let client = Self {
            inner: Arc::new(NinePClientInner {
                transport,
                msize: AtomicU32::new(DEFAULT_MSIZE),
                next_tag: AtomicU16::new(1),
                next_fid: AtomicU32::new(2),
                root_fid: 1,
            }),
        };
        match client.call(NinePRequest::Version {
            msize: DEFAULT_MSIZE,
            version: VERSION.to_string(),
        })? {
            NinePResponse::Version { msize, version } if version == VERSION => {
                client.inner.msize.store(msize, Ordering::Relaxed);
            }
            _ => return Err(ErrorKind::Invalid.into()),
        }
        match client.call(NinePRequest::Attach {
            fid: client.inner.root_fid,
            afid: NOFID,
            uname: "wanix".to_string(),
            aname: String::new(),
            n_uname: 0,
        })? {
            NinePResponse::Attach { .. } => Ok(client),
            _ => Err(ErrorKind::Invalid.into()),
        }
    }

    fn call(&self, request: NinePRequest) -> Result<NinePResponse> {
        let tag = self.alloc_tag();
        let frame = encode_request(tag, &request)?;
        let response = self.inner.transport.round_trip(frame)?;
        let (response_tag, response) = decode_response(&response)?;
        if response_tag != tag {
            return Err(ErrorKind::Invalid.into());
        }
        match response {
            NinePResponse::Lerror { ecode } => Err(errno_to_error(ecode)),
            response => Ok(response),
        }
    }

    fn alloc_tag(&self) -> u16 {
        let tag = self.inner.next_tag.fetch_add(1, Ordering::Relaxed);
        if tag == NOTAG {
            self.inner.next_tag.fetch_add(1, Ordering::Relaxed)
        } else {
            tag
        }
    }

    fn alloc_fid(&self) -> u32 {
        self.inner.next_fid.fetch_add(1, Ordering::Relaxed)
    }

    fn walk_fid(&self, name: &str) -> Result<u32> {
        let name = validated_client_path("walk", name)?;
        let fid = self.alloc_fid();
        let names = if name == "." {
            Vec::new()
        } else {
            name.split('/').map(ToString::to_string).collect()
        };
        let expected_qids = names.len();
        match self.call(NinePRequest::Walk {
            fid: self.inner.root_fid,
            newfid: fid,
            names,
        })? {
            NinePResponse::Walk { qids } if qids.len() == expected_qids => Ok(fid),
            NinePResponse::Walk { .. } => {
                let _ = self.clunk_fid(fid);
                Err(Error::path("walk", name, ErrorKind::NotFound))
            }
            _ => Err(ErrorKind::Invalid.into()),
        }
    }

    fn clunk_fid(&self, fid: u32) -> Result<()> {
        match self.call(NinePRequest::Clunk { fid })? {
            NinePResponse::Clunk => Ok(()),
            _ => Err(ErrorKind::Invalid.into()),
        }
    }

    fn getattr_fid(&self, fid: u32, path: &str) -> Result<Metadata> {
        match self.call(NinePRequest::GetAttr {
            fid,
            request_mask: ATTR_BASIC,
        })? {
            NinePResponse::GetAttr { attr } => Ok(metadata_from_attr(path, &attr)),
            _ => Err(ErrorKind::Invalid.into()),
        }
    }

    fn open_existing(&self, name: &str, flags: u32) -> Result<NinePFile> {
        let path = validated_client_path("open", name)?;
        let fid = self.walk_fid(&path)?;
        match self.call(NinePRequest::Lopen { fid, flags }) {
            Ok(NinePResponse::Lopen { .. }) => Ok(NinePFile::new(self.clone(), fid, path)),
            Ok(_) => {
                let _ = self.clunk_fid(fid);
                Err(ErrorKind::Invalid.into())
            }
            Err(err) => {
                let _ = self.clunk_fid(fid);
                Err(err)
            }
        }
    }

    fn set_attr(&self, path: &str, attr: SetAttr) -> Result<()> {
        let fid = self.walk_fid(path)?;
        let result = match self.call(NinePRequest::SetAttr { fid, attr }) {
            Ok(NinePResponse::SetAttr) => Ok(()),
            Ok(_) => Err(ErrorKind::Invalid.into()),
            Err(err) => Err(err),
        };
        let clunk = self.clunk_fid(fid);
        result.and(clunk)
    }

    fn xattrwalk_fid(&self, path: &str, attr: &str) -> Result<(u32, u64)> {
        let fid = self.walk_fid(path)?;
        let newfid = self.alloc_fid();
        let result = match self.call(NinePRequest::XattrWalk {
            fid,
            newfid,
            name: attr.to_string(),
        }) {
            Ok(NinePResponse::XattrWalk { size }) => Ok((newfid, size)),
            Ok(_) => Err(ErrorKind::Invalid.into()),
            Err(err) => Err(err),
        };
        let clunk = self.clunk_fid(fid);
        match (result, clunk) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(err), Ok(())) | (Err(err), Err(_)) => {
                let _ = self.clunk_fid(newfid);
                Err(err)
            }
            (Ok(_), Err(err)) => {
                let _ = self.clunk_fid(newfid);
                Err(err)
            }
        }
    }

    fn read_all_fid(&self, fid: u32, size: u64) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        let mut offset = 0_u64;
        let count = self
            .inner
            .msize
            .load(Ordering::Relaxed)
            .saturating_sub(11)
            .max(1);
        while offset < size {
            let remaining = size - offset;
            let chunk = match self.call(NinePRequest::Read {
                fid,
                offset,
                count: count.min(remaining.min(u64::from(u32::MAX)) as u32),
            })? {
                NinePResponse::Read { data } => data,
                _ => return Err(ErrorKind::Invalid.into()),
            };
            if chunk.is_empty() {
                return Err(ErrorKind::UnexpectedEof.into());
            }
            offset = offset
                .checked_add(chunk.len().try_into().map_err(|_| ErrorKind::Invalid)?)
                .ok_or(ErrorKind::Invalid)?;
            data.extend_from_slice(&chunk);
        }
        Ok(data)
    }

    fn write_all_fid(&self, fid: u32, data: &[u8]) -> Result<()> {
        let count = self
            .inner
            .msize
            .load(Ordering::Relaxed)
            .saturating_sub(23)
            .max(1) as usize;
        let mut offset = 0_u64;
        for chunk in data.chunks(count) {
            match self.call(NinePRequest::Write {
                fid,
                offset,
                data: chunk.to_vec(),
            })? {
                NinePResponse::Write { count } if count as usize == chunk.len() => {
                    offset = offset
                        .checked_add(chunk.len().try_into().map_err(|_| ErrorKind::Invalid)?)
                        .ok_or(ErrorKind::Invalid)?;
                }
                NinePResponse::Write { .. } => return Err(ErrorKind::UnexpectedEof.into()),
                _ => return Err(ErrorKind::Invalid.into()),
            }
        }
        Ok(())
    }
}

impl FileSystem for NinePClientFs {
    fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
        match self.open_existing(name, OpenFlags::RDWR.bits()) {
            Ok(file) => Ok(Box::new(file)),
            Err(_) => Ok(Box::new(
                self.open_existing(name, OpenFlags::RDONLY.bits())?,
            )),
        }
    }

    fn stat(&self, _ctx: &FsContext, name: &str) -> Result<Metadata> {
        let name = validated_client_path("stat", name)?;
        let fid = self.walk_fid(&name)?;
        let result = self.getattr_fid(fid, &name);
        let clunk = self.clunk_fid(fid);
        let meta = result?;
        clunk?;
        Ok(meta)
    }

    fn lstat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        self.stat(ctx, name)
    }

    fn read_dir(&self, _ctx: &FsContext, name: &str) -> Result<Vec<DirEntry>> {
        let mut file = self.open_existing(name, OpenFlags::RDONLY.bits())?;
        let entries = file.read_dir(-1);
        let close = file.close();
        let entries = entries?;
        close?;
        Ok(entries)
    }

    fn create(&self, name: &str) -> Result<BoxFile> {
        self.open_file(
            name,
            OpenFlags::RDWR | OpenFlags::CREATE | OpenFlags::TRUNC,
            FileMode::from_perm(0o644),
        )
    }

    fn open_file(&self, name: &str, flags: OpenFlags, perm: FileMode) -> Result<BoxFile> {
        let path = validated_client_path("open", name)?;
        if path == "." {
            return Err(Error::path("open", path, ErrorKind::Invalid));
        }
        if flags.contains(OpenFlags::CREATE) {
            let parent = parent_path(&path);
            let fid = self.walk_fid(&parent)?;
            match self.call(NinePRequest::Lcreate {
                fid,
                name: base_name(&path).to_string(),
                flags: flags.bits(),
                mode: perm.perm(),
                gid: 0,
            }) {
                Ok(NinePResponse::Lcreate { .. }) => {
                    let mut file = NinePFile::new(self.clone(), fid, path);
                    if flags.contains(OpenFlags::APPEND) {
                        file.seek(SeekFrom::End(0))?;
                    }
                    Ok(Box::new(file))
                }
                Ok(_) => {
                    let _ = self.clunk_fid(fid);
                    Err(ErrorKind::Invalid.into())
                }
                Err(err) => {
                    let _ = self.clunk_fid(fid);
                    Err(err)
                }
            }
        } else {
            let mut file = self.open_existing(&path, flags.bits())?;
            if flags.contains(OpenFlags::TRUNC) {
                self.set_attr(
                    &path,
                    SetAttr {
                        valid: SETATTR_SIZE,
                        size: 0,
                        ..SetAttr::default()
                    },
                )?;
            }
            if flags.contains(OpenFlags::APPEND) {
                file.seek(SeekFrom::End(0))?;
            }
            Ok(Box::new(file))
        }
    }

    fn mkdir(&self, name: &str, perm: FileMode) -> Result<()> {
        let path = validated_client_path("mkdir", name)?;
        if path == "." {
            return Err(Error::path("mkdir", path, ErrorKind::Invalid));
        }
        let parent = parent_path(&path);
        let fid = self.walk_fid(&parent)?;
        let result = match self.call(NinePRequest::Mkdir {
            fid,
            name: base_name(&path).to_string(),
            mode: perm.perm(),
            gid: 0,
        }) {
            Ok(NinePResponse::Mkdir { .. }) => Ok(()),
            Ok(_) => Err(ErrorKind::Invalid.into()),
            Err(err) => Err(err),
        };
        let clunk = self.clunk_fid(fid);
        result.and(clunk)
    }

    fn remove(&self, name: &str) -> Result<()> {
        let path = validated_client_path("remove", name)?;
        if path == "." {
            return Err(Error::path("remove", path, ErrorKind::Invalid));
        }
        let meta = self.stat(&FsContext::new().no_follow(), &path)?;
        let parent = parent_path(&path);
        let fid = self.walk_fid(&parent)?;
        let result = match self.call(NinePRequest::UnlinkAt {
            dirfid: fid,
            name: base_name(&path).to_string(),
            flags: if meta.is_dir() { AT_REMOVEDIR } else { 0 },
        }) {
            Ok(NinePResponse::UnlinkAt) => Ok(()),
            Ok(_) => Err(ErrorKind::Invalid.into()),
            Err(err) => Err(err),
        };
        let clunk = self.clunk_fid(fid);
        result.and(clunk)
    }

    fn rename(&self, old: &str, new: &str) -> Result<()> {
        let old = validated_client_path("rename", old)?;
        let new = validated_client_path("rename", new)?;
        let old_parent = self.walk_fid(&parent_path(&old))?;
        let new_parent = self.walk_fid(&parent_path(&new))?;
        let result = match self.call(NinePRequest::RenameAt {
            olddirfid: old_parent,
            oldname: base_name(&old).to_string(),
            newdirfid: new_parent,
            newname: base_name(&new).to_string(),
        }) {
            Ok(NinePResponse::RenameAt) => Ok(()),
            Ok(_) => Err(ErrorKind::Invalid.into()),
            Err(err) => Err(err),
        };
        let old_clunk = self.clunk_fid(old_parent);
        let new_clunk = self.clunk_fid(new_parent);
        result.and(old_clunk).and(new_clunk)
    }

    fn link(&self, old: &str, new: &str) -> Result<()> {
        let old = validated_client_path("link", old)?;
        let new = validated_client_path("link", new)?;
        if old == "." || new == "." {
            return Err(Error::path("link", new, ErrorKind::Invalid));
        }
        let old_fid = self.walk_fid(&old)?;
        let new_parent = self.walk_fid(&parent_path(&new))?;
        let result = match self.call(NinePRequest::Link {
            fid: old_fid,
            newdirfid: new_parent,
            name: base_name(&new).to_string(),
        }) {
            Ok(NinePResponse::Link) => Ok(()),
            Ok(_) => Err(ErrorKind::Invalid.into()),
            Err(err) => Err(err),
        };
        let old_clunk = self.clunk_fid(old_fid);
        let new_clunk = self.clunk_fid(new_parent);
        result.and(old_clunk).and(new_clunk)
    }

    fn chmod(&self, name: &str, mode: FileMode) -> Result<()> {
        self.set_attr(
            name,
            SetAttr {
                valid: SETATTR_MODE,
                mode: mode.perm(),
                ..SetAttr::default()
            },
        )
    }

    fn chown(&self, name: &str, uid: u32, gid: u32) -> Result<()> {
        self.set_attr(
            name,
            SetAttr {
                valid: SETATTR_UID | SETATTR_GID,
                uid,
                gid,
                ..SetAttr::default()
            },
        )
    }

    fn chtimes(&self, name: &str, mtime: SystemTime) -> Result<()> {
        let (secs, nanos) = system_time_parts(mtime);
        self.set_attr(
            name,
            SetAttr {
                valid: SETATTR_MTIME | SETATTR_MTIME_SET,
                mtime_seconds: secs,
                mtime_nanoseconds: nanos as u64,
                ..SetAttr::default()
            },
        )
    }

    fn truncate(&self, name: &str, size: u64) -> Result<()> {
        self.set_attr(
            name,
            SetAttr {
                valid: SETATTR_SIZE,
                size,
                ..SetAttr::default()
            },
        )
    }

    fn symlink(&self, old: &str, new: &str) -> Result<()> {
        let new = validated_client_path("symlink", new)?;
        let parent = self.walk_fid(&parent_path(&new))?;
        let result = match self.call(NinePRequest::Symlink {
            fid: parent,
            name: base_name(&new).to_string(),
            target: old.to_string(),
            gid: 0,
        }) {
            Ok(NinePResponse::Symlink { .. }) => Ok(()),
            Ok(_) => Err(ErrorKind::Invalid.into()),
            Err(err) => Err(err),
        };
        let clunk = self.clunk_fid(parent);
        result.and(clunk)
    }

    fn readlink(&self, name: &str) -> Result<String> {
        let fid = self.walk_fid(name)?;
        let result: Result<String> = match self.call(NinePRequest::Readlink { fid }) {
            Ok(NinePResponse::Readlink { target }) => Ok(target),
            Ok(_) => Err(ErrorKind::Invalid.into()),
            Err(err) => Err(err),
        };
        let clunk = self.clunk_fid(fid);
        let target = result?;
        clunk?;
        Ok(target)
    }

    fn set_xattr(&self, name: &str, attr: &str, data: &[u8]) -> Result<()> {
        let path = validated_client_path("setxattr", name)?;
        let fid = self.walk_fid(&path)?;
        let result = match self.call(NinePRequest::XattrCreate {
            fid,
            name: attr.to_string(),
            size: data.len().try_into().map_err(|_| ErrorKind::Invalid)?,
            flags: 0,
        }) {
            Ok(NinePResponse::XattrCreate) => self.write_all_fid(fid, data),
            Ok(_) => Err(ErrorKind::Invalid.into()),
            Err(err) => Err(err),
        };
        let clunk = self.clunk_fid(fid);
        result.and(clunk)
    }

    fn get_xattr(&self, name: &str, attr: &str) -> Result<Vec<u8>> {
        let path = validated_client_path("getxattr", name)?;
        let (fid, size) = self.xattrwalk_fid(&path, attr)?;
        let result = self.read_all_fid(fid, size);
        let clunk = self.clunk_fid(fid);
        let data = result?;
        clunk?;
        Ok(data)
    }

    fn list_xattrs(&self, name: &str) -> Result<Vec<String>> {
        let path = validated_client_path("listxattrs", name)?;
        let (fid, size) = self.xattrwalk_fid(&path, "")?;
        let result = self
            .read_all_fid(fid, size)
            .and_then(|data| decode_xattr_names(&data));
        let clunk = self.clunk_fid(fid);
        let attrs = result?;
        clunk?;
        Ok(attrs)
    }
}

struct NinePFile {
    client: NinePClientFs,
    fid: u32,
    path: String,
    offset: u64,
    closed: bool,
    dir_cache: Option<Vec<DirEntry>>,
    dir_offset: usize,
}

impl NinePFile {
    fn new(client: NinePClientFs, fid: u32, path: String) -> Self {
        Self {
            client,
            fid,
            path,
            offset: 0,
            closed: false,
            dir_cache: None,
            dir_offset: 0,
        }
    }

    fn ensure_open(&self) -> Result<()> {
        if self.closed {
            Err(ErrorKind::Closed.into())
        } else {
            Ok(())
        }
    }
}

impl Drop for NinePFile {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.client.clunk_fid(self.fid);
            self.closed = true;
        }
    }
}

impl fs::FileHandle for NinePFile {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.ensure_open()?;
        let data = match self.client.call(NinePRequest::Read {
            fid: self.fid,
            offset: self.offset,
            count: buf.len().try_into().map_err(|_| ErrorKind::Invalid)?,
        })? {
            NinePResponse::Read { data } => data,
            _ => return Err(ErrorKind::Invalid.into()),
        };
        let n = data.len();
        buf[..n].copy_from_slice(&data);
        self.offset += n as u64;
        Ok(n)
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.ensure_open()?;
        let count = match self.client.call(NinePRequest::Write {
            fid: self.fid,
            offset: self.offset,
            data: data.to_vec(),
        })? {
            NinePResponse::Write { count } => count as usize,
            _ => return Err(ErrorKind::Invalid.into()),
        };
        self.offset += count as u64;
        Ok(count)
    }

    fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize> {
        self.ensure_open()?;
        let data = match self.client.call(NinePRequest::Read {
            fid: self.fid,
            offset,
            count: buf.len().try_into().map_err(|_| ErrorKind::Invalid)?,
        })? {
            NinePResponse::Read { data } => data,
            _ => return Err(ErrorKind::Invalid.into()),
        };
        let n = data.len();
        buf[..n].copy_from_slice(&data);
        Ok(n)
    }

    fn write_at(&mut self, data: &[u8], offset: u64) -> Result<usize> {
        self.ensure_open()?;
        match self.client.call(NinePRequest::Write {
            fid: self.fid,
            offset,
            data: data.to_vec(),
        })? {
            NinePResponse::Write { count } => Ok(count as usize),
            _ => Err(ErrorKind::Invalid.into()),
        }
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.ensure_open()?;
        let next = match pos {
            SeekFrom::Start(pos) => pos as i128,
            SeekFrom::Current(delta) => self.offset as i128 + delta as i128,
            SeekFrom::End(delta) => self.stat()?.size as i128 + delta as i128,
        };
        if next < 0 {
            return Err(ErrorKind::Invalid.into());
        }
        self.offset = next as u64;
        Ok(self.offset)
    }

    fn stat(&self) -> Result<Metadata> {
        self.ensure_open()?;
        self.client.getattr_fid(self.fid, &self.path)
    }

    fn read_dir(&mut self, count: isize) -> Result<Vec<DirEntry>> {
        self.ensure_open()?;
        if self.dir_cache.is_none() {
            let mut offset = 0;
            let mut entries = Vec::new();
            let frame_count = self
                .client
                .inner
                .msize
                .load(Ordering::Relaxed)
                .saturating_sub(24);
            loop {
                let data = match self.client.call(NinePRequest::Readdir {
                    fid: self.fid,
                    offset,
                    count: frame_count,
                })? {
                    NinePResponse::Readdir { data } => data,
                    _ => return Err(ErrorKind::Invalid.into()),
                };
                if data.is_empty() {
                    break;
                }
                let dirents = decode_dirents(&data)?;
                let Some(last) = dirents.last() else {
                    break;
                };
                offset = last.offset;
                entries.extend(dirents.into_iter().map(|dirent| {
                    DirEntry::new(
                        dirent.name.clone(),
                        Metadata {
                            name: dirent.name,
                            mode: mode_from_dirent_type(dirent.typ),
                            size: 0,
                            modified: SystemTime::UNIX_EPOCH,
                            uid: 0,
                            gid: 0,
                        },
                    )
                }));
            }
            self.dir_cache = Some(entries);
        }
        let entries = self.dir_cache.as_ref().unwrap();
        if count < 0 {
            self.dir_offset = entries.len();
            return Ok(entries.clone());
        }
        if self.dir_offset >= entries.len() {
            return Ok(Vec::new());
        }
        let end = if count == 0 {
            entries.len()
        } else {
            (self.dir_offset + count as usize).min(entries.len())
        };
        let out = entries[self.dir_offset..end].to_vec();
        self.dir_offset = end;
        Ok(out)
    }

    fn sync(&mut self) -> Result<()> {
        self.ensure_open()?;
        match self.client.call(NinePRequest::Fsync { fid: self.fid })? {
            NinePResponse::Fsync => Ok(()),
            _ => Err(ErrorKind::Invalid.into()),
        }
    }

    fn close(&mut self) -> Result<()> {
        if self.closed {
            return Err(ErrorKind::Closed.into());
        }
        self.client.clunk_fid(self.fid)?;
        self.closed = true;
        Ok(())
    }
}

fn qid_for(fsys: &dyn FileSystem, path: &str) -> Result<Qid> {
    let meta = fsys.lstat(&FsContext::new().no_follow(), path)?;
    Ok(qid_from_meta(path, &meta))
}

fn qid_from_meta(path: &str, meta: &Metadata) -> Qid {
    let typ = if meta.mode.is_dir() {
        QTDIR
    } else if meta.mode.is_symlink() {
        QTSYMLINK
    } else {
        0
    };
    Qid {
        typ,
        version: 0,
        path: fnv1a64(path.as_bytes()),
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn attr_for(fsys: &dyn FileSystem, path: &str, meta: &Metadata, request_mask: u64) -> NinePAttr {
    let (secs, nanos) = system_time_parts(meta.modified);
    let blocks = meta.size.div_ceil(65_536);
    NinePAttr {
        valid: request_mask & ATTR_BASIC,
        qid: qid_from_meta(path, meta),
        mode: meta.mode.unix_type_and_perm(),
        uid: meta.uid,
        gid: meta.gid,
        nlink: if meta.is_dir() {
            2 + fsys
                .read_dir(&FsContext::new(), path)
                .map(|entries| entries.len() as u64)
                .unwrap_or(0)
        } else {
            1
        },
        rdev: 0,
        size: meta.size,
        blksize: 65_536,
        blocks,
        atime_seconds: secs,
        atime_nanoseconds: nanos as u64,
        mtime_seconds: secs,
        mtime_nanoseconds: nanos as u64,
        ctime_seconds: secs,
        ctime_nanoseconds: nanos as u64,
        btime_seconds: 0,
        btime_nanoseconds: 0,
        gen: 0,
        data_version: 0,
    }
}

fn metadata_from_attr(path: &str, attr: &NinePAttr) -> Metadata {
    Metadata {
        name: base_name(path).to_string(),
        mode: mode_from_unix(attr.mode),
        size: attr.size,
        modified: SystemTime::UNIX_EPOCH
            + Duration::new(attr.mtime_seconds, attr.mtime_nanoseconds as u32),
        uid: attr.uid,
        gid: attr.gid,
    }
}

fn mode_from_unix(mode: u32) -> FileMode {
    let kind = match mode & 0o170000 {
        0o040000 => FileMode::DIR,
        0o120000 => FileMode::SYMLINK,
        0o010000 => FileMode::NAMED_PIPE,
        0o140000 => FileMode::SOCKET,
        _ => FileMode::empty(),
    };
    kind | FileMode::from_perm(mode & 0o777)
}

fn mode_from_dirent_type(typ: u8) -> FileMode {
    match typ {
        DT_DIR => FileMode::DIR | FileMode::from_perm(0o755),
        DT_LNK => FileMode::SYMLINK | FileMode::from_perm(0o777),
        _ => FileMode::from_perm(0o644),
    }
}

fn dirent_type(mode: FileMode) -> u8 {
    if mode.is_dir() {
        DT_DIR
    } else if mode.is_symlink() {
        DT_LNK
    } else {
        DT_REG
    }
}

fn encode_xattr_names(names: &[String]) -> Vec<u8> {
    let mut names = names.to_vec();
    names.sort();
    let mut out = Vec::new();
    for name in &names {
        out.extend_from_slice(name.as_bytes());
        out.push(0);
    }
    out
}

fn decode_xattr_names(data: &[u8]) -> Result<Vec<String>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    if data.last().copied() != Some(0) {
        return Err(ErrorKind::Invalid.into());
    }
    data[..data.len() - 1]
        .split(|byte| *byte == 0)
        .map(|name| String::from_utf8(name.to_vec()).map_err(|_| ErrorKind::Invalid.into()))
        .collect()
}

fn open_flags_from_9p(flags: u32) -> OpenFlags {
    let mut out = match flags & 0b11 {
        1 => OpenFlags::WRONLY,
        2 => OpenFlags::RDWR,
        _ => OpenFlags::RDONLY,
    };
    for (bit, flag) in [
        (OpenFlags::CREATE.bits(), OpenFlags::CREATE),
        (OpenFlags::EXCL.bits(), OpenFlags::EXCL),
        (OpenFlags::TRUNC.bits(), OpenFlags::TRUNC),
        (OpenFlags::APPEND.bits(), OpenFlags::APPEND),
    ] {
        if flags & bit != 0 {
            out |= flag;
        }
    }
    out
}

fn walk_join(base: &str, name: &str) -> String {
    match name {
        "." => base.to_string(),
        ".." => parent_path(base),
        _ => join_path(base, name),
    }
}

fn checked_join(base: &str, name: &str) -> Result<String> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(ErrorKind::Invalid.into());
    }
    Ok(join_path(base, name))
}

fn join_path(base: &str, name: &str) -> String {
    if base == "." {
        clean_path(name)
    } else {
        clean_path(&format!("{base}/{name}"))
    }
}

fn system_time_parts(time: SystemTime) -> (u64, u32) {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    (duration.as_secs(), duration.subsec_nanos())
}

fn current_time() -> SystemTime {
    #[cfg(target_arch = "wasm32")]
    {
        SystemTime::UNIX_EPOCH
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        SystemTime::now()
    }
}

fn errno_for_error(err: &Error) -> u32 {
    match err.kind() {
        ErrorKind::NotFound => ENOENT,
        ErrorKind::AlreadyExists => EEXIST,
        ErrorKind::Invalid => EINVAL,
        ErrorKind::PermissionDenied => EACCES,
        ErrorKind::NotSupported => ENOSYS,
        ErrorKind::NotEmpty => ENOTEMPTY,
        ErrorKind::Closed => EBADF,
        ErrorKind::NotDir => ENOTDIR,
        ErrorKind::IsDir => EISDIR,
        ErrorKind::UnexpectedEof => EIO,
        ErrorKind::Other => EIO,
    }
}

fn errno_to_error(errno: u32) -> Error {
    match errno {
        EPERM => ErrorKind::PermissionDenied.into(),
        ENOENT => ErrorKind::NotFound.into(),
        EBADF => ErrorKind::Invalid.into(),
        EACCES => ErrorKind::PermissionDenied.into(),
        EEXIST => ErrorKind::AlreadyExists.into(),
        ENOTDIR => ErrorKind::NotDir.into(),
        EISDIR => ErrorKind::IsDir.into(),
        EINVAL => ErrorKind::Invalid.into(),
        ENOSYS => ErrorKind::NotSupported.into(),
        ENOTEMPTY => ErrorKind::NotEmpty.into(),
        _ => ErrorKind::Other.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wanix_fs::{directory_file, fs_ref, MemFs};

    fn round_trip_request(request: NinePRequest) {
        let encoded = encode_request(42, &request).unwrap();
        assert_eq!(decode_request(&encoded).unwrap(), (42, request));
    }

    fn round_trip_response(response: NinePResponse) {
        let encoded = encode_response(42, &response).unwrap();
        assert_eq!(decode_response(&encoded).unwrap(), (42, response));
    }

    fn client_with_memfs() -> (MemFs, NinePClientFs) {
        let mem = MemFs::from_entries([("dir/file.txt", b"hello".as_slice())]);
        let transport = Arc::new(LoopbackTransport::with_filesystem(fs_ref(mem.clone())));
        let client = NinePClientFs::connect(transport).unwrap();
        (mem, client)
    }

    fn fid_snapshot(server: &NinePServer) -> Vec<(u32, String, bool, u32)> {
        let mut fids: Vec<_> = server
            .state
            .lock()
            .unwrap()
            .fids
            .iter()
            .map(|(fid, state)| {
                (
                    *fid,
                    state.path.clone(),
                    matches!(&state.handle, FidHandle::File(_)),
                    state.flags,
                )
            })
            .collect();
        fids.sort_by_key(|(fid, ..)| *fid);
        fids
    }

    fn async_exchange(server: &AsyncNinePServer, tag: u16, request: NinePRequest) -> NinePResponse {
        let frame = encode_request(tag, &request).unwrap();
        let response = match server.handle_frame(&frame).unwrap() {
            Some(response) => response,
            None => server
                .recv_response_timeout(Duration::from_secs(2))
                .expect("async 9P response"),
        };
        let (response_tag, response) = decode_response(&response).unwrap();
        assert_eq!(response_tag, tag);
        response
    }

    #[derive(Clone)]
    struct BlockingReadFs {
        state: Arc<BlockingReadState>,
    }

    struct BlockingReadState {
        started: (Mutex<bool>, Condvar),
        released: (Mutex<bool>, Condvar),
    }

    impl BlockingReadFs {
        fn new() -> Self {
            Self {
                state: Arc::new(BlockingReadState {
                    started: (Mutex::new(false), Condvar::new()),
                    released: (Mutex::new(false), Condvar::new()),
                }),
            }
        }

        fn wait_started(&self) {
            let (lock, ready) = &self.state.started;
            let mut started = lock.lock().unwrap();
            while !*started {
                started = ready.wait(started).unwrap();
            }
        }

        fn release(&self) {
            let (lock, ready) = &self.state.released;
            *lock.lock().unwrap() = true;
            ready.notify_all();
        }
    }

    impl FileSystem for BlockingReadFs {
        fn open(&self, _ctx: &FsContext, name: &str) -> Result<BoxFile> {
            match clean_path(name).as_str() {
                "." => Ok(directory_file(
                    Metadata::dir(".", 0o555),
                    vec![DirEntry::new(
                        "slow.txt",
                        Metadata::file("slow.txt", 0o444, "slow-data".len() as u64),
                    )],
                )),
                "slow.txt" => Ok(Box::new(BlockingReadFile {
                    state: self.state.clone(),
                    data: b"slow-data".to_vec(),
                })),
                _ => Err(ErrorKind::NotFound.into()),
            }
        }
    }

    struct BlockingReadFile {
        state: Arc<BlockingReadState>,
        data: Vec<u8>,
    }

    impl FileHandle for BlockingReadFile {
        fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize> {
            let (started_lock, started_ready) = &self.state.started;
            *started_lock.lock().unwrap() = true;
            started_ready.notify_all();

            let (released_lock, released_ready) = &self.state.released;
            let mut released = released_lock.lock().unwrap();
            while !*released {
                released = released_ready.wait(released).unwrap();
            }

            let start: usize = offset.try_into().map_err(|_| ErrorKind::Invalid)?;
            if start >= self.data.len() {
                return Ok(0);
            }
            let count = buf.len().min(self.data.len() - start);
            buf[..count].copy_from_slice(&self.data[start..start + count]);
            Ok(count)
        }

        fn stat(&self) -> Result<Metadata> {
            Ok(Metadata::file(
                "slow.txt",
                0o444,
                self.data.len().try_into().unwrap(),
            ))
        }
    }

    #[test]
    fn codec_round_trips_core_messages() {
        round_trip_request(NinePRequest::Version {
            msize: DEFAULT_MSIZE,
            version: VERSION.to_string(),
        });
        round_trip_request(NinePRequest::Walk {
            fid: 1,
            newfid: 2,
            names: vec!["dir".to_string(), "file.txt".to_string()],
        });
        round_trip_request(NinePRequest::Write {
            fid: 2,
            offset: 7,
            data: b"abc".to_vec(),
        });
        round_trip_request(NinePRequest::XattrWalk {
            fid: 2,
            newfid: 3,
            name: "user.mime_type".to_string(),
        });
        round_trip_request(NinePRequest::XattrCreate {
            fid: 2,
            name: "user.author".to_string(),
            size: 5,
            flags: 0,
        });
        round_trip_request(NinePRequest::Link {
            fid: 2,
            newdirfid: 3,
            name: "linked".to_string(),
        });
        round_trip_request(NinePRequest::RenameAt {
            olddirfid: 1,
            oldname: "a".to_string(),
            newdirfid: 3,
            newname: "b".to_string(),
        });
        round_trip_request(NinePRequest::Flush { oldtag: 17 });

        let qid = Qid {
            typ: QTDIR,
            version: 0,
            path: 99,
        };
        round_trip_response(NinePResponse::Attach { qid });
        round_trip_response(NinePResponse::Read {
            data: b"payload".to_vec(),
        });
        round_trip_response(NinePResponse::XattrWalk { size: 12 });
        round_trip_response(NinePResponse::XattrCreate);
        round_trip_response(NinePResponse::Link);
        round_trip_response(NinePResponse::GetAttr {
            attr: NinePAttr {
                valid: ATTR_BASIC,
                qid,
                mode: 0o040755,
                uid: 1,
                gid: 2,
                nlink: 2,
                rdev: 0,
                size: 2,
                blksize: 65_536,
                blocks: 1,
                atime_seconds: 1,
                atime_nanoseconds: 2,
                mtime_seconds: 3,
                mtime_nanoseconds: 4,
                ctime_seconds: 5,
                ctime_nanoseconds: 6,
                btime_seconds: 0,
                btime_nanoseconds: 0,
                gen: 0,
                data_version: 0,
            },
        });
        round_trip_response(NinePResponse::Flush);
    }

    #[test]
    fn stream_helpers_preserve_consecutive_frame_boundaries() {
        let first = encode_request(
            1,
            &NinePRequest::Version {
                msize: DEFAULT_MSIZE,
                version: VERSION.to_string(),
            },
        )
        .unwrap();
        let second = encode_request(2, &NinePRequest::Flush { oldtag: 1 }).unwrap();
        let mut stream = Vec::new();
        write_frame(&mut stream, &first).unwrap();
        write_frame(&mut stream, &second).unwrap();

        let mut reader = std::io::Cursor::new(stream);
        assert_eq!(read_frame(&mut reader).unwrap(), Some(first));
        assert_eq!(read_frame(&mut reader).unwrap(), Some(second));
        assert_eq!(read_frame(&mut reader).unwrap(), None);
    }

    #[test]
    fn stream_helpers_reject_invalid_frame_sizes() {
        let mut too_short = std::io::Cursor::new(3_u32.to_le_bytes());
        assert_eq!(
            read_frame(&mut too_short).unwrap_err().kind(),
            ErrorKind::Invalid
        );

        let mut mismatched = encode_request(1, &NinePRequest::Flush { oldtag: 1 }).unwrap();
        let declared_size = mismatched.len() as u32 + 1;
        mismatched[..4].copy_from_slice(&declared_size.to_le_bytes());
        assert_eq!(
            write_frame(&mut Vec::new(), &mismatched)
                .unwrap_err()
                .kind(),
            ErrorKind::Invalid
        );
    }

    #[test]
    fn serve_frame_stream_handles_multiple_requests() {
        let fs = fs_ref(MemFs::from_entries([("file.txt", b"hello".to_vec())]));
        let server = NinePServer::new(fs);
        let requests = [
            encode_request(
                1,
                &NinePRequest::Version {
                    msize: DEFAULT_MSIZE,
                    version: VERSION.to_string(),
                },
            )
            .unwrap(),
            encode_request(
                2,
                &NinePRequest::Attach {
                    fid: 1,
                    afid: NOFID,
                    uname: "wanix".to_string(),
                    aname: String::new(),
                    n_uname: 0,
                },
            )
            .unwrap(),
            encode_request(3, &NinePRequest::Flush { oldtag: 2 }).unwrap(),
        ]
        .concat();
        let mut reader = std::io::Cursor::new(requests);
        let mut responses = Vec::new();

        assert_eq!(
            serve_frame_stream(&server, &mut reader, &mut responses).unwrap(),
            3
        );

        let mut response_reader = std::io::Cursor::new(responses);
        assert!(matches!(
            read_frame(&mut response_reader)
                .unwrap()
                .map(|frame| decode_response(&frame).unwrap()),
            Some((1, NinePResponse::Version { .. }))
        ));
        assert!(matches!(
            read_frame(&mut response_reader)
                .unwrap()
                .map(|frame| decode_response(&frame).unwrap()),
            Some((2, NinePResponse::Attach { .. }))
        ));
        assert_eq!(
            read_frame(&mut response_reader)
                .unwrap()
                .map(|frame| decode_response(&frame).unwrap()),
            Some((3, NinePResponse::Flush))
        );
        assert_eq!(read_frame(&mut response_reader).unwrap(), None);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stream_transport_imports_namespace_over_tcp_listener() {
        use std::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = NinePServer::new(fs_ref(MemFs::from_entries([(
            "dir/file.txt",
            b"hello".to_vec(),
        )])));
        let served = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = stream.try_clone().unwrap();
            let mut writer = stream;
            serve_frame_stream(&server, &mut reader, &mut writer).unwrap()
        });

        let stream = TcpStream::connect(addr).unwrap();
        let client = NinePClientFs::connect(Arc::new(TcpStreamTransport::new(stream))).unwrap();
        assert_eq!(fs::read_file(&client, "dir/file.txt").unwrap(), b"hello");
        fs::write_file(
            &client,
            "dir/created.txt",
            b"created",
            FileMode::from_perm(0o644),
        )
        .unwrap();
        assert_eq!(
            fs::read_file(&client, "dir/created.txt").unwrap(),
            b"created"
        );
        drop(client);
        assert!(served.join().unwrap() >= 4);
    }

    #[test]
    fn raw_server_handles_attach_walk_and_getattr() {
        let mem = MemFs::from_entries([("dir/file.txt", b"hello".as_slice())]);
        let server = NinePServer::new(fs_ref(mem));

        let version = server
            .handle_frame(
                &encode_request(
                    1,
                    &NinePRequest::Version {
                        msize: DEFAULT_MSIZE,
                        version: VERSION.to_string(),
                    },
                )
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            decode_response(&version).unwrap().1,
            NinePResponse::Version { .. }
        ));

        let attach = server
            .handle_frame(
                &encode_request(
                    2,
                    &NinePRequest::Attach {
                        fid: 1,
                        afid: NOFID,
                        uname: "u".into(),
                        aname: String::new(),
                        n_uname: 0,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            decode_response(&attach).unwrap().1,
            NinePResponse::Attach { .. }
        ));

        let walk = server
            .handle_frame(
                &encode_request(
                    3,
                    &NinePRequest::Walk {
                        fid: 1,
                        newfid: 2,
                        names: vec!["dir".into(), "file.txt".into()],
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let (_, response) = decode_response(&walk).unwrap();
        match response {
            NinePResponse::Walk { qids } => assert_eq!(qids.len(), 2),
            other => panic!("unexpected response: {other:?}"),
        }

        let getattr = server
            .handle_frame(
                &encode_request(
                    4,
                    &NinePRequest::GetAttr {
                        fid: 2,
                        request_mask: ATTR_BASIC,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let (_, response) = decode_response(&getattr).unwrap();
        match response {
            NinePResponse::GetAttr { attr } => {
                assert_eq!(attr.size, 5);
                assert_eq!(attr.mode & 0o170000, 0o100000);
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn raw_server_partial_walk_returns_successful_prefix_only() {
        let mem = MemFs::from_entries([("dir/file.txt", b"hello".as_slice())]);
        let server = NinePServer::new(fs_ref(mem));
        server
            .handle_frame(
                &encode_request(
                    1,
                    &NinePRequest::Attach {
                        fid: 1,
                        afid: NOFID,
                        uname: "u".into(),
                        aname: String::new(),
                        n_uname: 0,
                    },
                )
                .unwrap(),
            )
            .unwrap();

        let walk = server
            .handle_frame(
                &encode_request(
                    2,
                    &NinePRequest::Walk {
                        fid: 1,
                        newfid: 2,
                        names: vec!["dir".into(), "missing.txt".into()],
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let (_, response) = decode_response(&walk).unwrap();
        match response {
            NinePResponse::Walk { qids } => assert_eq!(qids.len(), 1),
            other => panic!("unexpected response: {other:?}"),
        }

        let getattr = server
            .handle_frame(
                &encode_request(
                    3,
                    &NinePRequest::GetAttr {
                        fid: 2,
                        request_mask: ATTR_BASIC,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let (_, response) = decode_response(&getattr).unwrap();
        match response {
            NinePResponse::GetAttr { attr } => assert_ne!(attr.mode & 0o040000, 0),
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn raw_server_rejects_walk_over_existing_newfid() {
        let mem = MemFs::from_entries([("dir/file.txt", b"hello".as_slice())]);
        let server = NinePServer::new(fs_ref(mem));
        server
            .handle_frame(
                &encode_request(
                    1,
                    &NinePRequest::Attach {
                        fid: 1,
                        afid: NOFID,
                        uname: "u".into(),
                        aname: String::new(),
                        n_uname: 0,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        server
            .handle_frame(
                &encode_request(
                    2,
                    &NinePRequest::Walk {
                        fid: 1,
                        newfid: 2,
                        names: vec!["dir".into()],
                    },
                )
                .unwrap(),
            )
            .unwrap();

        let walk = server
            .handle_frame(
                &encode_request(
                    3,
                    &NinePRequest::Walk {
                        fid: 1,
                        newfid: 2,
                        names: vec!["dir".into()],
                    },
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            decode_response(&walk).unwrap().1,
            NinePResponse::Lerror { ecode: EINVAL }
        );
    }

    #[test]
    fn raw_server_flush_acknowledges_without_mutating_fids() {
        let mem = MemFs::from_entries([("dir/file.txt", b"hello".as_slice())]);
        let server = NinePServer::new(fs_ref(mem));
        server
            .handle_frame(
                &encode_request(
                    1,
                    &NinePRequest::Attach {
                        fid: 1,
                        afid: NOFID,
                        uname: "u".into(),
                        aname: String::new(),
                        n_uname: 0,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        server
            .handle_frame(
                &encode_request(
                    2,
                    &NinePRequest::Walk {
                        fid: 1,
                        newfid: 2,
                        names: vec!["dir".into()],
                    },
                )
                .unwrap(),
            )
            .unwrap();

        let before = fid_snapshot(&server);
        let flush = server
            .handle_frame(&encode_request(9, &NinePRequest::Flush { oldtag: 2 }).unwrap())
            .unwrap();
        assert_eq!(decode_response(&flush).unwrap(), (9, NinePResponse::Flush));
        assert_eq!(fid_snapshot(&server), before);

        let getattr = server
            .handle_frame(
                &encode_request(
                    10,
                    &NinePRequest::GetAttr {
                        fid: 2,
                        request_mask: ATTR_BASIC,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let (_, response) = decode_response(&getattr).unwrap();
        match response {
            NinePResponse::GetAttr { attr } => assert_ne!(attr.mode & 0o040000, 0),
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn async_server_flush_cancels_pending_read_and_suppresses_late_reply() {
        let blocking = BlockingReadFs::new();
        let server = AsyncNinePServer::new(fs_ref(blocking.clone()));

        assert!(matches!(
            async_exchange(
                &server,
                1,
                NinePRequest::Version {
                    msize: DEFAULT_MSIZE,
                    version: VERSION.to_string(),
                },
            ),
            NinePResponse::Version { .. }
        ));
        assert!(matches!(
            async_exchange(
                &server,
                2,
                NinePRequest::Attach {
                    fid: 1,
                    afid: NOFID,
                    uname: "u".into(),
                    aname: String::new(),
                    n_uname: 0,
                },
            ),
            NinePResponse::Attach { .. }
        ));
        assert!(matches!(
            async_exchange(
                &server,
                3,
                NinePRequest::Walk {
                    fid: 1,
                    newfid: 2,
                    names: vec!["slow.txt".into()],
                },
            ),
            NinePResponse::Walk { qids } if qids.len() == 1
        ));
        assert!(matches!(
            async_exchange(&server, 4, NinePRequest::Lopen { fid: 2, flags: 0 }),
            NinePResponse::Lopen { .. }
        ));

        let read = encode_request(
            5,
            &NinePRequest::Read {
                fid: 2,
                offset: 0,
                count: 9,
            },
        )
        .unwrap();
        assert!(server.handle_frame(&read).unwrap().is_none());
        blocking.wait_started();
        assert_eq!(server.pending_tags(), vec![5]);

        assert_eq!(
            async_exchange(&server, 6, NinePRequest::Flush { oldtag: 5 }),
            NinePResponse::Flush
        );
        assert!(server.pending_tags().is_empty());

        blocking.release();
        assert!(server
            .recv_response_timeout(Duration::from_millis(150))
            .is_none());
        assert_eq!(
            async_exchange(&server, 7, NinePRequest::Clunk { fid: 2 }),
            NinePResponse::Clunk
        );
    }

    #[test]
    fn async_server_duplicate_tag_cancels_previous_pending_operation() {
        let blocking = BlockingReadFs::new();
        let server = AsyncNinePServer::new(fs_ref(blocking.clone()));

        async_exchange(
            &server,
            1,
            NinePRequest::Attach {
                fid: 1,
                afid: NOFID,
                uname: "u".into(),
                aname: String::new(),
                n_uname: 0,
            },
        );
        async_exchange(
            &server,
            2,
            NinePRequest::Walk {
                fid: 1,
                newfid: 2,
                names: vec!["slow.txt".into()],
            },
        );
        async_exchange(&server, 3, NinePRequest::Lopen { fid: 2, flags: 0 });

        let read = encode_request(
            4,
            &NinePRequest::Read {
                fid: 2,
                offset: 0,
                count: 9,
            },
        )
        .unwrap();
        assert!(server.handle_frame(&read).unwrap().is_none());
        blocking.wait_started();

        let duplicate = encode_request(
            4,
            &NinePRequest::GetAttr {
                fid: 2,
                request_mask: ATTR_BASIC,
            },
        )
        .unwrap();
        let duplicate_response = server.handle_frame(&duplicate).unwrap().unwrap();
        assert_eq!(
            decode_response(&duplicate_response).unwrap(),
            (4, NinePResponse::Lerror { ecode: EINVAL })
        );
        assert!(server.pending_tags().is_empty());

        blocking.release();
        assert!(server
            .recv_response_timeout(Duration::from_millis(150))
            .is_none());
    }

    #[test]
    fn client_filesystem_reads_writes_and_lists_memfs() {
        let (_mem, client) = client_with_memfs();

        assert_eq!(fs::read_file(&client, "dir/file.txt").unwrap(), b"hello");
        fs::write_file(
            &client,
            "dir/created.txt",
            b"created",
            FileMode::from_perm(0o644),
        )
        .unwrap();
        assert_eq!(
            fs::read_file(&client, "dir/created.txt").unwrap(),
            b"created"
        );

        let mut file = client
            .open_file("dir/created.txt", OpenFlags::RDWR, FileMode::empty())
            .unwrap();
        assert_eq!(file.write_at(b"XX", 2).unwrap(), 2);
        file.close().unwrap();
        assert_eq!(
            fs::read_file(&client, "dir/created.txt").unwrap(),
            b"crXXted"
        );

        let entries: Vec<_> = fs::read_dir(&client, "dir")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(entries, vec!["created.txt", "file.txt"]);
    }

    #[test]
    fn client_filesystem_renames_and_removes() {
        let (_mem, client) = client_with_memfs();

        client.rename("dir/file.txt", "dir/moved.txt").unwrap();
        assert_eq!(fs::read_file(&client, "dir/moved.txt").unwrap(), b"hello");
        assert_eq!(
            fs::stat(&client, "dir/file.txt").unwrap_err().kind(),
            ErrorKind::NotFound
        );

        client.remove("dir/moved.txt").unwrap();
        assert_eq!(
            fs::stat(&client, "dir/moved.txt").unwrap_err().kind(),
            ErrorKind::NotFound
        );
    }

    #[test]
    fn client_filesystem_creates_hard_links() {
        let (mem, client) = client_with_memfs();

        client.link("dir/file.txt", "dir/linked.txt").unwrap();
        let mut linked = client
            .open_file("dir/linked.txt", OpenFlags::RDWR, FileMode::empty())
            .unwrap();
        linked.write(b"HELLO").unwrap();
        linked.close().unwrap();

        assert_eq!(fs::read_file(&client, "dir/file.txt").unwrap(), b"HELLO");
        client.remove("dir/file.txt").unwrap();
        assert_eq!(fs::read_file(&client, "dir/linked.txt").unwrap(), b"HELLO");
        assert_eq!(fs::read_file(&mem, "dir/linked.txt").unwrap(), b"HELLO");
    }

    #[test]
    fn client_filesystem_creates_and_removes_directories() {
        let (_mem, client) = client_with_memfs();

        client
            .mkdir("empty", FileMode::DIR | FileMode::from_perm(0o755))
            .unwrap();
        assert!(fs::stat(&client, "empty").unwrap().is_dir());
        client.remove("empty").unwrap();
        assert_eq!(
            fs::stat(&client, "empty").unwrap_err().kind(),
            ErrorKind::NotFound
        );
    }

    #[test]
    fn client_treats_partial_walk_as_not_found() {
        let (_mem, client) = client_with_memfs();

        assert_eq!(
            fs::stat(&client, "dir/missing.txt").unwrap_err().kind(),
            ErrorKind::NotFound
        );

        assert_eq!(fs::read_file(&client, "dir/file.txt").unwrap(), b"hello");
    }

    #[test]
    fn raw_server_reads_xattr_via_xattrwalk() {
        let mem = MemFs::from_entries([("dir/file.txt", b"hello".as_slice())]);
        fs::set_xattr(&mem, "dir/file.txt", "user.mime_type", b"text/plain").unwrap();
        let server = NinePServer::new(fs_ref(mem));
        server
            .handle_frame(
                &encode_request(
                    1,
                    &NinePRequest::Attach {
                        fid: 1,
                        afid: NOFID,
                        uname: "u".into(),
                        aname: String::new(),
                        n_uname: 0,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        server
            .handle_frame(
                &encode_request(
                    2,
                    &NinePRequest::Walk {
                        fid: 1,
                        newfid: 2,
                        names: vec!["dir".into(), "file.txt".into()],
                    },
                )
                .unwrap(),
            )
            .unwrap();

        let walk = server
            .handle_frame(
                &encode_request(
                    3,
                    &NinePRequest::XattrWalk {
                        fid: 2,
                        newfid: 3,
                        name: "user.mime_type".into(),
                    },
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            decode_response(&walk).unwrap(),
            (
                3,
                NinePResponse::XattrWalk {
                    size: b"text/plain".len() as u64,
                },
            )
        );

        let read = server
            .handle_frame(
                &encode_request(
                    4,
                    &NinePRequest::Read {
                        fid: 3,
                        offset: 0,
                        count: 64,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            decode_response(&read).unwrap(),
            (
                4,
                NinePResponse::Read {
                    data: b"text/plain".to_vec(),
                },
            )
        );
    }

    #[test]
    fn raw_server_lists_xattrs_via_empty_xattrwalk_name() {
        let mem = MemFs::from_entries([("dir/file.txt", b"hello".as_slice())]);
        fs::set_xattr(&mem, "dir/file.txt", "user.author", b"wanix").unwrap();
        fs::set_xattr(&mem, "dir/file.txt", "user.mime_type", b"text/plain").unwrap();
        let server = NinePServer::new(fs_ref(mem));
        server
            .handle_frame(
                &encode_request(
                    1,
                    &NinePRequest::Attach {
                        fid: 1,
                        afid: NOFID,
                        uname: "u".into(),
                        aname: String::new(),
                        n_uname: 0,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        server
            .handle_frame(
                &encode_request(
                    2,
                    &NinePRequest::Walk {
                        fid: 1,
                        newfid: 2,
                        names: vec!["dir".into(), "file.txt".into()],
                    },
                )
                .unwrap(),
            )
            .unwrap();

        let expected = b"user.author\0user.mime_type\0".to_vec();
        let walk = server
            .handle_frame(
                &encode_request(
                    3,
                    &NinePRequest::XattrWalk {
                        fid: 2,
                        newfid: 3,
                        name: String::new(),
                    },
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            decode_response(&walk).unwrap(),
            (
                3,
                NinePResponse::XattrWalk {
                    size: expected.len() as u64,
                },
            )
        );

        let read = server
            .handle_frame(
                &encode_request(
                    4,
                    &NinePRequest::Read {
                        fid: 3,
                        offset: 0,
                        count: 64,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            decode_response(&read).unwrap(),
            (4, NinePResponse::Read { data: expected })
        );
    }

    #[test]
    fn raw_server_commits_xattr_writes_on_clunk() {
        let mem = MemFs::from_entries([("dir/file.txt", b"hello".as_slice())]);
        let server = NinePServer::new(fs_ref(mem.clone()));
        server
            .handle_frame(
                &encode_request(
                    1,
                    &NinePRequest::Attach {
                        fid: 1,
                        afid: NOFID,
                        uname: "u".into(),
                        aname: String::new(),
                        n_uname: 0,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        server
            .handle_frame(
                &encode_request(
                    2,
                    &NinePRequest::Walk {
                        fid: 1,
                        newfid: 2,
                        names: vec!["dir".into(), "file.txt".into()],
                    },
                )
                .unwrap(),
            )
            .unwrap();

        let create = server
            .handle_frame(
                &encode_request(
                    3,
                    &NinePRequest::XattrCreate {
                        fid: 2,
                        name: "user.note".into(),
                        size: 5,
                        flags: 0,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            decode_response(&create).unwrap(),
            (3, NinePResponse::XattrCreate)
        );

        let write = server
            .handle_frame(
                &encode_request(
                    4,
                    &NinePRequest::Write {
                        fid: 2,
                        offset: 0,
                        data: b"hello".to_vec(),
                    },
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            decode_response(&write).unwrap(),
            (4, NinePResponse::Write { count: 5 })
        );
        assert_eq!(
            fs::get_xattr(&mem, "dir/file.txt", "user.note")
                .unwrap_err()
                .kind(),
            ErrorKind::NotFound
        );

        let clunk = server
            .handle_frame(&encode_request(5, &NinePRequest::Clunk { fid: 2 }).unwrap())
            .unwrap();
        assert_eq!(decode_response(&clunk).unwrap(), (5, NinePResponse::Clunk));
        assert_eq!(
            fs::get_xattr(&mem, "dir/file.txt", "user.note").unwrap(),
            b"hello"
        );
    }

    #[test]
    fn client_filesystem_round_trips_xattrs() {
        let (mem, client) = client_with_memfs();
        fs::set_xattr(&mem, "dir/file.txt", "user.mime_type", b"text/plain").unwrap();

        assert_eq!(
            fs::get_xattr(&client, "dir/file.txt", "user.mime_type").unwrap(),
            b"text/plain"
        );
        assert_eq!(
            fs::list_xattrs(&client, "dir/file.txt").unwrap(),
            vec!["user.mime_type".to_string()]
        );

        fs::set_xattr(&client, "dir/file.txt", "user.author", b"wanix").unwrap();
        assert_eq!(
            fs::list_xattrs(&client, "dir/file.txt").unwrap(),
            vec!["user.author".to_string(), "user.mime_type".to_string()]
        );
        assert_eq!(
            fs::get_xattr(&mem, "dir/file.txt", "user.author").unwrap(),
            b"wanix"
        );
    }

    #[test]
    fn client_rejects_invalid_paths_like_reference() {
        let (_mem, client) = client_with_memfs();

        for path in ["", "../etc/passwd", "foo/../../../bar"] {
            let err = match client.open(&FsContext::new(), path) {
                Ok(_) => panic!("unexpected open success for {path}"),
                Err(err) => err,
            };
            assert_eq!(err.kind(), ErrorKind::Invalid);
            assert_eq!(
                client
                    .mkdir(path, FileMode::from_perm(0o755))
                    .unwrap_err()
                    .kind(),
                ErrorKind::Invalid
            );
            assert_eq!(client.remove(path).unwrap_err().kind(), ErrorKind::Invalid);
        }
    }
}
