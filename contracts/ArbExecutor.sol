// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

interface IERC20 {
    function transfer(address to, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
}

interface IUniswapV2Pair {
    function swap(uint amount0Out, uint amount1Out, address to, bytes calldata data) external;
    function token0() external view returns (address);
    function token1() external view returns (address);
    function getReserves() external view returns (uint112, uint112, uint32);
}

/// @title ArbExecutor
/// @notice Minimal atomic cross-DEX arbitrage executor for V2-style pools.
/// @dev Owner sends token_in to this contract, then calls executeArb().
///      The contract swaps through the specified pairs atomically and
///      reverts if profit is below minProfit (ensuring no loss).
/// @title ArbExecutor
/// @notice Atomic cross-DEX arbitrage executor for V2-style pools.
/// @dev Fund this contract with tokenIn, then call executeArb().
///      Reverts if profit < minProfit (no loss possible).
contract ArbExecutor {
    address public immutable owner;

    modifier onlyOwner() {
        require(msg.sender == owner, "not owner");
        _;
    }

    constructor() {
        owner = msg.sender;
    }

    /// @notice Execute a 2-hop cross-DEX arbitrage atomically.
    /// @param tokenIn   The input token (also the output token — circular arb).
    /// @param tokenMid  The intermediate token.
    /// @param pairBuy   The V2 pair to buy tokenMid (swap tokenIn -> tokenMid).
    /// @param pairSell  The V2 pair to sell tokenMid (swap tokenMid -> tokenIn).
    /// @param amountIn  Amount of tokenIn to spend.
    /// @param minProfit Minimum profit in tokenIn units; reverts if not met.
    function executeArb(
        address tokenIn,
        address tokenMid,
        address pairBuy,
        address pairSell,
        uint256 amountIn,
        uint256 minProfit
    ) external onlyOwner {
        uint256 balBefore = IERC20(tokenIn).balanceOf(address(this));
        require(balBefore >= amountIn, "insufficient balance");

        // --- Hop 1: swap tokenIn -> tokenMid on pairBuy ---
        uint256 midAmount = _swapOnPair(pairBuy, tokenIn, tokenMid, amountIn);

        // --- Hop 2: swap tokenMid -> tokenIn on pairSell ---
        _swapOnPair(pairSell, tokenMid, tokenIn, midAmount);

        // --- Profit check ---
        uint256 balAfter = IERC20(tokenIn).balanceOf(address(this));
        require(balAfter >= balBefore + minProfit, "insufficient profit");
    }

    /// @dev Perform a swap on a V2-style pair using the low-level swap() function.
    ///      Sends tokenIn to the pair first, then calls swap().
    function _swapOnPair(
        address pair,
        address tokenIn,
        address tokenOut,
        uint256 amountIn
    ) internal returns (uint256 amountOut) {
        // Determine swap direction.
        address token0 = IUniswapV2Pair(pair).token0();
        bool isToken0In = (tokenIn == token0);

        // Calculate amount out using on-chain reserves (authoritative).
        (uint112 r0, uint112 r1, ) = IUniswapV2Pair(pair).getReserves();
        (uint256 reserveIn, uint256 reserveOut) = isToken0In
            ? (uint256(r0), uint256(r1))
            : (uint256(r1), uint256(r0));

        uint256 amountInWithFee = amountIn * 997;
        amountOut = (amountInWithFee * reserveOut) / (reserveIn * 1000 + amountInWithFee);
        require(amountOut > 0, "zero output");

        // Transfer tokenIn to the pair.
        IERC20(tokenIn).transfer(pair, amountIn);

        // Execute the swap.
        (uint256 amount0Out, uint256 amount1Out) = isToken0In
            ? (uint256(0), amountOut)
            : (amountOut, uint256(0));

        IUniswapV2Pair(pair).swap(amount0Out, amount1Out, address(this), "");
    }

    /// @notice Withdraw any token from the contract (emergency / profit withdrawal).
    function withdraw(address token, uint256 amount) external onlyOwner {
        IERC20(token).transfer(owner, amount);
    }

    /// @notice Withdraw ETH (if any).
    function withdrawETH() external onlyOwner {
        (bool ok, ) = owner.call{value: address(this).balance}("");
        require(ok, "eth transfer failed");
    }

    receive() external payable {}
}
