use std::io;


pub trait ReadExt {
    fn read_u32_le(&mut self) -> Result<u32, io::Error>;
    fn read_nul_terminated_byte_string(&mut self) -> Result<Vec<u8>, io::Error>;
}
impl<R: io::Read> ReadExt for R {
    fn read_u32_le(&mut self) -> Result<u32, io::Error> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    fn read_nul_terminated_byte_string(&mut self) -> Result<Vec<u8>, io::Error> {
        let mut buf = [0];
        let mut ret = Vec::new();
        loop {
            self.read_exact(&mut buf)?;
            if buf[0] == 0 {
                break;
            }
            ret.push(buf[0]);
        }
        Ok(ret)
    }
}
