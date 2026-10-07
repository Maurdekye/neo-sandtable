import { expect, it } from 'vitest'
import { decodeMessage } from './wire'
it('rejects malformed OA parents and detail fields before hierarchy rendering', () => {
  const unit = {
    id: 'u',
    side: 'axis',
    name: 'Fixture',
    kind: 'infantry',
    size: 'battalion',
    nationality: 'synthetic',
    hex: null,
    parent: null,
    detail: null,
  }
  const packet = (value: unknown) =>
    JSON.stringify({
      type: 'event',
      seq: 1,
      clock: { game_turn: 1, date: '', stage: 'setup', phase: 'setup' },
      event: { kind: 'unit_updated', unit: value },
    })
  expect(decodeMessage(packet(unit))).toBeTruthy()
  expect(() =>
    decodeMessage(packet({ ...unit, parent: { secret: 'invalid' } })),
  ).toThrow()
  expect(() =>
    decodeMessage(packet({ ...unit, detail: ['invalid'] })),
  ).toThrow()
})

it('rejects malformed optional envelope locators but accepts absent ones',()=>{
 const m={type:'event',seq:1,clock:{game_turn:1,date:'',stage:'setup',phase:'setup'},event:{kind:'note',text:'Fixture'}}
 expect(decodeMessage(JSON.stringify(m))).toBeTruthy()
 expect(decodeMessage(JSON.stringify({...m,hex:'C4218',unit_id:'u'}))).toBeTruthy()
 expect(()=>decodeMessage(JSON.stringify({...m,hex:42}))).toThrow()
 expect(()=>decodeMessage(JSON.stringify({...m,unit_id:{id:'private'}}))).toThrow()
})
