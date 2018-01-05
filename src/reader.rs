#![cfg(not(feature="no-stdlib"))]
pub use alloc::{AllocatedStackMemory, Allocator, SliceWrapper, SliceWrapperMut, StackAllocator};
pub use alloc::HeapAlloc;
use std::io;
use std::io::{Read};
use super::BrotliResult;
use ::interface::{Compressor, DivansCompressorFactory, Decompressor};
use ::DivansDecompressorFactory;
use ::brotli;
use ::interface;

trait Processor {
   fn process(&mut self, input:&[u8], input_offset:&mut usize, output:&mut [u8], output_offset:&mut usize) -> BrotliResult;
   fn close(&mut self, output:&mut [u8], output_offset:&mut usize) -> BrotliResult;
}

struct GenReader<R: Read,
                 P:Processor,
                 BufferType:SliceWrapperMut<u8>> {
  compressor: P,
  input_buffer: BufferType,
  input_offset: usize,
  input_len: usize,
  input_eof: bool,
  has_flushed: bool,
  input: R,
  read_error: Option<io::Error>,
}


impl<R:Read, P:Processor, BufferType:SliceWrapperMut<u8>> Read for GenReader<R,P,BufferType> {
	fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        let mut output_offset : usize = 0;
        let mut avail_out = buf.len() - output_offset;
        let mut avail_in = self.input_len - self.input_offset;
        let mut needs_input = false;
        while avail_out == buf.len() && (!needs_input || !self.input_eof) {
           if self.input_len < self.input_buffer.slice_mut().len() && !self.input_eof {
               match self.input.read(&mut self.input_buffer.slice_mut()[self.input_len..]) {
                   Err(e) => {
                       if let io::ErrorKind::Interrupted = e.kind() {
                          continue;
                       }
                       self.read_error = Some(e);
                       self.input_eof = true;
                   },
                   Ok(size) => if size == 0 {
                       self.input_eof = true;
                   } else {
                       needs_input = false;
                       self.input_len += size;
                       avail_in = self.input_len - self.input_offset;
                   },
               }
           }
           let old_output_offset = output_offset;
           let old_input_offset = self.input_offset;
           let ret = if avail_in == 0 {
               self.has_flushed = true;
               self.compressor.close(
                   buf.split_at_mut(output_offset + avail_out).0,
                   &mut output_offset)
           } else {
               self.compressor.process(
                   self.input_buffer.slice_mut().split_at_mut(self.input_offset + avail_in).0,
                   &mut self.input_offset,
                   buf.split_at_mut(output_offset + avail_out).0,
                   &mut output_offset)
           };
           avail_in -= self.input_offset - old_input_offset;
           avail_out -= output_offset - old_output_offset;
           if avail_in == 0 {
             match self.read_error.take() {
               Some(err) => return Err(err),
               None => {
                 needs_input = true;
                 self.copy_to_front();
               },
             }
           }
           match ret {
             BrotliResult::ResultFailure => return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid Data")),
             BrotliResult::ResultSuccess => {
               if self.input_eof && avail_in == 0 && self.has_flushed {
                 break;
               }
             },
             BrotliResult::NeedsMoreInput | BrotliResult::NeedsMoreOutput => {},
           }
        }
        Ok(output_offset)
    }
}
impl<R:Read, C:Processor, BufferType:SliceWrapperMut<u8>> GenReader<R,C,BufferType>{
    pub fn new(reader:R, compressor:C, buffer:BufferType, needs_flush: bool) ->Self {
        GenReader {
            input:reader,
            compressor:compressor,
            input_buffer: buffer,
            input_offset : 0,
            input_len : 0,
            input_eof : false,
            has_flushed: !needs_flush,
            read_error: None,
        }
    }
    pub fn copy_to_front(&mut self) {
        let avail_in = self.input_len - self.input_offset;
        if self.input_offset == self.input_buffer.slice_mut().len() {
            self.input_offset = 0;
            self.input_len = 0;
        } else if self.input_offset + 256 > self.input_buffer.slice_mut().len() && avail_in < self.input_offset {
            let (first, second) = self.input_buffer.slice_mut().split_at_mut(self.input_offset);
            first[0..avail_in].clone_from_slice(&second[0..avail_in]);
            self.input_len -= self.input_offset;
            self.input_offset = 0;
        }
    }
}
type DivansBrotliFactory = ::BrotliDivansHybridCompressorFactory<HeapAlloc<u8>,
                                                         HeapAlloc<u16>,
                                                         HeapAlloc<u32>,
                                                         HeapAlloc<i32>,
                                                         HeapAlloc<brotli::enc::command::Command>,
                                                         HeapAlloc<::CDF2>,
                                                         HeapAlloc<::DefaultCDF16>,
                                                         HeapAlloc<brotli::enc::util::floatX>,
                                                         HeapAlloc<brotli::enc::vectorization::Mem256f>,
                                                         HeapAlloc<brotli::enc::histogram::HistogramLiteral>,
                                                         HeapAlloc<brotli::enc::histogram::HistogramCommand>,
                                                         HeapAlloc<brotli::enc::histogram::HistogramDistance>,
                                                         HeapAlloc<brotli::enc::cluster::HistogramPair>,
                                                         HeapAlloc<brotli::enc::histogram::ContextType>,
                                                         HeapAlloc<brotli::enc::entropy_encode::HuffmanTree>>;
type DivansBrotliConstructedCompressor = <DivansBrotliFactory as ::DivansCompressorFactory<HeapAlloc<u8>,
                                                                                           HeapAlloc<u32>,
                                                                                           HeapAlloc<::CDF2>,
                                                                                           HeapAlloc<::DefaultCDF16>>>::ConstructedCompressor;
impl<T:Compressor> Processor for T {
   fn process(&mut self, input:&[u8], input_offset:&mut usize, output:&mut [u8], output_offset:&mut usize) -> BrotliResult {
       self.encode(input, input_offset, output, output_offset)
   }
   fn close(&mut self, output:&mut [u8], output_offset:&mut usize) -> BrotliResult{
      self.flush(output, output_offset)
   }

}
pub struct DivansBrotliHybridCompressorReader<R:Read>(GenReader<R,
                                                                DivansBrotliConstructedCompressor,
                                                                <HeapAlloc<u8> as Allocator<u8>>::AllocatedMemory,
                                                               >);
impl<R:Read> Read for DivansBrotliHybridCompressorReader<R> {
	fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        self.0.read(buf)
    }
}
impl<R:Read> DivansBrotliHybridCompressorReader<R> {
    pub fn new(reader: R, opts: interface::DivansCompressorOptions, mut buffer_size: usize) -> Self {
       if buffer_size == 0 {
          buffer_size = 4096;
       }
       let mut m8 = HeapAlloc::<u8>::new(0);
       let buffer = m8.alloc_cell(buffer_size);
       DivansBrotliHybridCompressorReader::<R>(
           GenReader::<R,
                       DivansBrotliConstructedCompressor,
                       <HeapAlloc<u8> as Allocator<u8>>::AllocatedMemory>::new(
                          reader,
                          DivansBrotliFactory::new(
                                           m8,
                                           HeapAlloc::<u32>::new(0),
                                           HeapAlloc::<::CDF2>::new(::CDF2::default()),
                                           HeapAlloc::<::DefaultCDF16>::new(::DefaultCDF16::default()),
                                           opts.window_size.unwrap_or(21) as usize,
                                           opts.dynamic_context_mixing.unwrap_or(0),
                                           opts.literal_adaptation,
                                           opts.use_context_map,
                                           opts.force_stride_value,
                                           (
                                               HeapAlloc::<u8>::new(0),
                                               HeapAlloc::<u16>::new(0),
                                               HeapAlloc::<i32>::new(0),
                                               HeapAlloc::<brotli::enc::command::Command>::new(brotli::enc::command::Command::default()),
                                               HeapAlloc::<brotli::enc::util::floatX>::new(0.0 as brotli::enc::util::floatX),
                                               HeapAlloc::<brotli::enc::vectorization::Mem256f>::new(brotli::enc::vectorization::Mem256f::default()),
                                               HeapAlloc::<brotli::enc::histogram::HistogramLiteral>::new(brotli::enc::histogram::HistogramLiteral::default()),
                                               HeapAlloc::<brotli::enc::histogram::HistogramCommand>::new(brotli::enc::histogram::HistogramCommand::default()),
                                               HeapAlloc::<brotli::enc::histogram::HistogramDistance>::new(brotli::enc::histogram::HistogramDistance::default()),
                                               HeapAlloc::<brotli::enc::cluster::HistogramPair>::new(brotli::enc::cluster::HistogramPair::default()),
                                               HeapAlloc::<brotli::enc::histogram::ContextType>::new(brotli::enc::histogram::ContextType::default()),
                                               HeapAlloc::<brotli::enc::entropy_encode::HuffmanTree>::new(brotli::enc::entropy_encode::HuffmanTree::default()),
                                               opts.quality,
                                               opts.lgblock
                                           )),
                          buffer,
                          true,
                       ))
    }
}


type DivansCustomFactory = ::DivansCompressorFactoryStruct<HeapAlloc<u8>,
                                                         HeapAlloc<::CDF2>,
                                                         HeapAlloc<::DefaultCDF16>>;
type DivansCustomConstructedCompressor = <DivansCustomFactory as ::DivansCompressorFactory<HeapAlloc<u8>,
                                                                                           HeapAlloc<u32>,
                                                                                           HeapAlloc<::CDF2>,
                                                                                           HeapAlloc<::DefaultCDF16>>>::ConstructedCompressor;
pub struct DivansExperimentalCompressorReader<R:Read>(GenReader<R,
                                                                DivansCustomConstructedCompressor,
                                                                 <HeapAlloc<u8> as Allocator<u8>>::AllocatedMemory,
                                                               >);
impl<R:Read> Read for DivansExperimentalCompressorReader<R> {
	fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        self.0.read(buf)
    }
}
impl<R:Read> DivansExperimentalCompressorReader<R> {
    pub fn new(reader: R, opts: interface::DivansCompressorOptions, mut buffer_size: usize) -> Self {
       if buffer_size == 0 {
          buffer_size = 4096;
       }
       let mut m8 = HeapAlloc::<u8>::new(0);
       let buffer = m8.alloc_cell(buffer_size);
       DivansExperimentalCompressorReader::<R>(
           GenReader::<R,
                       DivansCustomConstructedCompressor,
                       <HeapAlloc<u8> as Allocator<u8>>::AllocatedMemory>::new(
                          reader,
                          DivansCustomFactory::new(
                                           m8,
                                           HeapAlloc::<u32>::new(0),
                                           HeapAlloc::<::CDF2>::new(::CDF2::default()),
                                           HeapAlloc::<::DefaultCDF16>::new(::DefaultCDF16::default()),
                                           opts.window_size.unwrap_or(21) as usize,
                                           opts.dynamic_context_mixing.unwrap_or(0),
                                           opts.literal_adaptation,
                                           opts.use_context_map,
                                           opts.force_stride_value,
                                           ()),
                          buffer,
                          true,
                       ))
    }
}


type StandardDivansDecompressorFactory = ::DivansDecompressorFactoryStruct<HeapAlloc<u8>,
                                                                     HeapAlloc<::CDF2>,
                                                                     HeapAlloc<::DefaultCDF16>>;
type DivansConstructedDecompressor = ::DivansDecompressor<<StandardDivansDecompressorFactory as ::DivansDecompressorFactory<HeapAlloc<u8>,
                                                                                                       HeapAlloc<::CDF2>,
                                                                                                                            HeapAlloc<::DefaultCDF16>>
                                                           >::DefaultDecoder,
                                                          HeapAlloc<u8>,
                                                          HeapAlloc<::CDF2>,
                                                          HeapAlloc<::DefaultCDF16>>;
impl Processor for DivansConstructedDecompressor {
   fn process(&mut self, input:&[u8], input_offset:&mut usize, output:&mut [u8], output_offset:&mut usize) -> BrotliResult {
       self.decode(input, input_offset, output, output_offset)
   }
   fn close(&mut self, output:&mut [u8], output_offset:&mut usize) -> BrotliResult{
       let mut input_offset = 0usize;
       self.decode(&[], &mut input_offset, output, output_offset)
   }

}
pub struct DivansDecompressorReader<R:Read>(GenReader<R,
                                                      DivansConstructedDecompressor,
                                                      <HeapAlloc<u8> as Allocator<u8>>::AllocatedMemory,
                                                      >);
impl<R:Read> Read for DivansDecompressorReader<R> {
	fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        self.0.read(buf)
    }
}
impl<R:Read> DivansDecompressorReader<R> {
    pub fn new(reader: R, mut buffer_size: usize) -> Self {
       if buffer_size == 0 {
          buffer_size = 4096;
       }
       let mut m8 = HeapAlloc::<u8>::new(0);
       let buffer = m8.alloc_cell(buffer_size);
       DivansDecompressorReader::<R>(
           GenReader::<R,
                       DivansConstructedDecompressor,
                       <HeapAlloc<u8> as Allocator<u8>>::AllocatedMemory>::new(
                          reader,
                          StandardDivansDecompressorFactory::new(
                              m8,
                              HeapAlloc::<::CDF2>::new(::CDF2::default()),
                              HeapAlloc::<::DefaultCDF16>::new(::DefaultCDF16::default()),
                          ),
                          buffer,
                          false,
                       ))
    }
}

