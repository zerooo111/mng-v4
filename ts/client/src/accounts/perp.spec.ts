import { BN } from '@coral-xyz/anchor';
import { expect } from 'chai';
import { PerpMarket, PerpOrderSide } from './perp';

function testPerpMarket(params?: {
  baseDecimals?: number;
  baseLotSize?: number;
  quoteLotSize?: number;
}): PerpMarket {
  const baseDecimals = params?.baseDecimals ?? 6;
  const baseLotSize = params?.baseLotSize ?? 100;
  const quoteLotSize = params?.quoteLotSize ?? 10;

  const market = Object.create(PerpMarket.prototype) as PerpMarket;
  market.baseDecimals = baseDecimals;
  market.baseLotSize = new BN(baseLotSize);
  market.quoteLotSize = new BN(quoteLotSize);
  (market as any).priceLotsToUiConverter =
    (quoteLotSize * Math.pow(10, baseDecimals - 6)) / baseLotSize;
  return market;
}

describe('PerpMarket price-lot rounding', () => {
  it('derives a 0.01 tick by shrinking quote lot size instead of base decimals', () => {
    const market = testPerpMarket({
      baseDecimals: 6,
      baseLotSize: 100,
      quoteLotSize: 1,
    });

    expect(market.tickSize).eq(0.01);
    expect(market.priceLotsToUi(new BN(8743))).eq(87.43);
  });

  it('rounds bids down and asks up to the nearest price lot', () => {
    const market = testPerpMarket({
      baseDecimals: 6,
      baseLotSize: 100,
      quoteLotSize: 1,
    });

    expect(market.uiPriceToLotsForSide(87.431, PerpOrderSide.bid).toString()).eq(
      '8743',
    );
    expect(market.uiPriceToLotsForSide(87.431, PerpOrderSide.ask).toString()).eq(
      '8744',
    );
    expect(market.roundUiPriceToTick(87.431, PerpOrderSide.bid)).eq(87.43);
    expect(market.roundUiPriceToTick(87.431, PerpOrderSide.ask)).eq(87.44);
  });

  it('preserves exact tick-aligned prices for both sides', () => {
    const market = testPerpMarket({
      baseDecimals: 6,
      baseLotSize: 100,
      quoteLotSize: 1,
    });

    expect(market.uiPriceToLotsForSide(87.43, PerpOrderSide.bid).toString()).eq(
      '8743',
    );
    expect(market.uiPriceToLotsForSide(87.43, PerpOrderSide.ask).toString()).eq(
      '8743',
    );
  });
});
