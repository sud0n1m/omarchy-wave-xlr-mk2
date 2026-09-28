import unittest
from control import modify, decode, LENGTHS, FLAGS

class ProtocolTests(unittest.TestCase):
    def setUp(self):
        self.blocks={k:bytearray([0]*n) for k,n in LENGTHS.items()}
        self.blocks[4][0]=30
        self.blocks[4][1]=0b10101010
        self.blocks[4][2]=0b11111011
        self.blocks[5][0]=80
        self.blocks[1][0]=100
    def test_flags_preserve_every_other_bit_and_byte(self):
        for key in [*FLAGS,'clipguard','lowImpedance']:
            for value in (0,1):
                block,data=modify(self.blocks,key,value)
                offset,mask=(1,FLAGS[key]) if key in FLAGS else ((2,4) if key=='clipguard' else (1,2))
                for i in range(len(data)):
                    self.assertEqual(data[i] & (~mask if i==offset else 255),self.blocks[block][i] & (~mask if i==offset else 255))
                changed=dict(self.blocks); changed[block]=data
                self.assertEqual(decode(changed)[key],bool(value))
    def test_limits_and_units(self):
        for key,lo,hi in [('gain',0,80),('headphones',-60,0),('balance',0,200),('strength',0,100)]:
            for value in (lo,hi):
                block,data=modify(self.blocks,key,value)
                changed=dict(self.blocks); changed[block]=data
                self.assertEqual(decode(changed)[key],value)
            for value in (lo-1,hi+1):
                with self.assertRaises(ValueError): modify(self.blocks,key,value)
    def test_unexpected_firmware_state_refused(self):
        self.blocks[4][0]=255
        with self.assertRaises(ValueError): decode(self.blocks)

if __name__=='__main__': unittest.main()
